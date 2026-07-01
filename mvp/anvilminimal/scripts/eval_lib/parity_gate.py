from __future__ import annotations

import json
from pathlib import Path
from typing import Any

from .failure_classification import blank_failure_kind_gate_violations
from .runtime_trace import compare_trace_reports
from .run_summary import read_summary


REQUIRED_GATE_IDS = {f"G-S{index:02d}" for index in range(1, 17)}
GATE_PARTITION_FIELDS = (
    "passed_gate_ids",
    "partial_gate_ids",
    "failed_gate_ids",
    "intentionally_different_gate_ids",
)
DEFAULT_WARN_DELTA_PP = 5.0
DEFAULT_FAIL_DELTA_PP = 10.0


def validate_parity_gate_report(report: dict[str, Any]) -> list[str]:
    errors: list[str] = []
    if str(report.get("schema_version", "")) != "1":
        errors.append("schema_version must be 1")
    required = set(str(item) for item in report.get("required_gate_ids", []) or [])
    if required != REQUIRED_GATE_IDS:
        errors.append("required_gate_ids must be exactly G-S01..G-S16")

    seen: set[str] = set()
    for field in GATE_PARTITION_FIELDS:
        values = report.get(field, []) or []
        if not isinstance(values, list):
            errors.append(f"{field} must be a list")
            continue
        ids = set(str(item) for item in values)
        overlap = seen.intersection(ids)
        if overlap:
            errors.append(f"gate ids appear in multiple status lists: {sorted(overlap)}")
        seen.update(ids)
    if required and seen != required:
        missing = sorted(required.difference(seen))
        extra = sorted(seen.difference(required))
        if missing:
            errors.append(f"gate ids missing from status partition: {missing}")
        if extra:
            errors.append(f"unknown gate ids in status partition: {extra}")

    blank_count = int_or_none(report.get("failure_kind_blank_count"))
    if blank_count is None:
        errors.append("failure_kind_blank_count must be an integer")
    elif blank_count > 0:
        report_errors = report.get("errors", []) or []
        joined = "\n".join(str(item) for item in report_errors)
        if "failure_kind" not in joined:
            errors.append("blank failure kind count must be listed in report errors")

    gate_level = str(report.get("gate_level", ""))
    partial = set(str(item) for item in report.get("partial_gate_ids", []) or [])
    if gate_level in {"comparative", "release"} and partial:
        errors.append(
            "comparative/release gate cannot leave partial gate ids unresolved: "
            + str(sorted(partial))
        )
    gate_statuses = report.get("gate_statuses", []) or []
    allowed_gate_statuses = {"pass", "fail", "intentionally_different"}
    if gate_level not in {"comparative", "release"}:
        allowed_gate_statuses.add("partial")
    for item in gate_statuses:
        if not isinstance(item, dict):
            errors.append("gate_statuses entries must be objects")
            continue
        status = str(item.get("status", ""))
        if status not in allowed_gate_statuses:
            errors.append(
                "gate_statuses entries must resolve to pass/fail/intentionally_different"
            )
            break
    trace_diff = report.get("normalized_trace_diff", {}) or {}
    if gate_level in {"comparative", "release"}:
        if not trace_diff:
            report_errors = report.get("errors", []) or []
            joined = "\n".join(str(item) for item in report_errors)
            if "normalized trace" not in joined:
                errors.append("comparative/release gate requires normalized trace diff evidence")
        elif trace_diff.get("partial_gate_ids"):
            errors.append("normalized trace diff cannot contain partial gate ids")
        for item in trace_diff.get("gate_results", []) or []:
            status = str(item.get("status", ""))
            if status not in {"pass", "fail", "intentionally_different"}:
                errors.append(
                    "normalized trace diff gate results must resolve to pass/fail/intentionally_different"
                )
                break
    comparison = report.get("anvildev_comparison", {}) or {}
    if gate_level in {"comparative", "release"}:
        if comparison.get("status") == "missing_current_same_condition_trace":
            report_errors = report.get("errors", []) or []
            joined = "\n".join(str(item) for item in report_errors)
            if "anvildev" not in joined:
                errors.append("missing anvildev comparison must be listed in report errors")
        threshold = comparison.get("threshold", {}) or {}
        if threshold.get("status") == "fail":
            evidence = comparison.get("intentional_difference_evidence_paths", []) or []
            report_errors = report.get("errors", []) or []
            joined = "\n".join(str(item) for item in report_errors)
            if not evidence and "anvildev parity threshold" not in joined:
                errors.append(
                    "anvildev parity threshold failure must be an error or have intentional evidence"
                )
    uat = report.get("uat_equivalent", {}) or {}
    if gate_level == "release" and uat.get("status") == "pass":
        missing = release_evidence_gaps(uat)
        if missing:
            errors.append(
                "release gate cannot pass without required UAT evidence: "
                + ", ".join(missing)
            )
        blockers = release_evidence_blockers(uat)
        if blockers:
            errors.append(
                "release gate cannot pass with failing or invalid UAT evidence: "
                + ", ".join(blockers)
            )
    return errors


def blank_failure_kind_count(rows: list[dict[str, Any]]) -> int:
    return len(blank_failure_kind_gate_violations(rows))


def build_parity_gate_report(
    *,
    base_report: dict[str, Any] | None = None,
    gate_level: str = "local",
    mvp_summary_path: str | None = None,
    anvildev_summary_path: str | None = None,
    source_trace_report_path: str | None = None,
    mvp_trace_report_path: str | None = None,
    trace_diff_path: str | None = None,
    uat_evidence_paths: list[str] | None = None,
    browser_evidence_paths: list[str] | None = None,
    interaction_evidence_paths: list[str] | None = None,
    tui_event_paths: list[str] | None = None,
    intentional_difference_evidence_paths: list[str] | None = None,
    warn_delta_pp: float = DEFAULT_WARN_DELTA_PP,
    fail_delta_pp: float = DEFAULT_FAIL_DELTA_PP,
) -> dict[str, Any]:
    report = dict(base_report or minimal_parity_gate_report())
    report["gate_level"] = gate_level
    report.setdefault("required_gate_ids", sorted(REQUIRED_GATE_IDS))
    for field in GATE_PARTITION_FIELDS:
        report.setdefault(field, [])
    report["errors"] = list(report.get("errors", []) or [])
    report["warnings"] = list(report.get("warnings", []) or [])
    report["rollback_policy"] = {
        "blank_failure_kind_must_remain_blocked": True,
        "build_only_false_positive_must_remain_blocked": True,
        "threshold_can_be_downgraded_only_to_warning": True,
    }
    if mvp_summary_path:
        report["mvp_summary"] = summary_snapshot(mvp_summary_path)
        report["failure_kind_blank_count"] = report["mvp_summary"]["blank_failure_kind_count"]
    if mvp_summary_path and anvildev_summary_path:
        report["anvildev_comparison"] = compare_mvp_to_anvildev(
            mvp_summary_path=mvp_summary_path,
            anvildev_summary_path=anvildev_summary_path,
            intentional_difference_evidence_paths=intentional_difference_evidence_paths or [],
            warn_delta_pp=warn_delta_pp,
            fail_delta_pp=fail_delta_pp,
        )
    elif gate_level in {"comparative", "release"}:
        report["anvildev_comparison"] = {
            "status": "missing_current_same_condition_trace",
            "success_rate_delta_pp": None,
            "stage_regressions": [],
            "required_next_trace": "Run MVP and anvildev --engine minimal with the same suite/model profile/modes/runs/provider limits.",
        }
        append_unique(
            report["errors"],
            "comparative gate cannot pass because latest same-condition anvildev trace is missing.",
        )
    report["normalized_trace_diff"] = normalized_trace_diff(
        source_trace_report_path=source_trace_report_path,
        mvp_trace_report_path=mvp_trace_report_path,
        trace_diff_path=trace_diff_path,
    )
    if gate_level in {"comparative", "release"} and not trace_diff_is_usable(
        report["normalized_trace_diff"]
    ):
        append_unique(
            report["errors"],
            "comparative gate cannot pass because normalized trace diff evidence is missing or incomplete.",
        )
    report["uat_equivalent"] = release_gate_evidence(
        existing=report.get("uat_equivalent", {}) or {},
        gate_level=gate_level,
        uat_evidence_paths=uat_evidence_paths or [],
        browser_evidence_paths=browser_evidence_paths or [],
        interaction_evidence_paths=interaction_evidence_paths or [],
        tui_event_paths=tui_event_paths or [],
    )
    if gate_level == "release" and release_evidence_gaps(report["uat_equivalent"]):
        append_unique(
            report["errors"],
            "release-grade UAT/browser interaction evidence is missing, so release gate cannot pass.",
        )
    if (
        report.get("anvildev_comparison", {})
        .get("threshold", {})
        .get("status")
        == "fail"
    ):
        append_unique(
            report["errors"],
            "anvildev parity threshold failed: MVP is at least 10pp below source without intentional evidence.",
        )
    finalize_gate_partition(report, gate_level=gate_level)
    if gate_level in {"comparative", "release"} and report.get("failed_gate_ids"):
        append_unique(
            report["errors"],
            "runtime semantics gates failed: " + ", ".join(report["failed_gate_ids"]),
        )
    return report


def minimal_parity_gate_report() -> dict[str, Any]:
    return {
        "schema_version": "1",
        "gate_level": "local",
        "required_gate_ids": sorted(REQUIRED_GATE_IDS),
        "passed_gate_ids": [],
        "partial_gate_ids": sorted(REQUIRED_GATE_IDS),
        "failed_gate_ids": [],
        "intentionally_different_gate_ids": [],
        "failure_kind_blank_count": 0,
        "errors": [],
        "warnings": [],
    }


def normalized_trace_diff(
    *,
    source_trace_report_path: str | None,
    mvp_trace_report_path: str | None,
    trace_diff_path: str | None,
) -> dict[str, Any]:
    if trace_diff_path:
        diff = read_json_path(trace_diff_path, {})
        if isinstance(diff, dict):
            diff.setdefault("trace_diff_path", trace_diff_path)
            return diff
        return {"schema_version": "1", "status": "invalid_trace_diff", "trace_diff_path": trace_diff_path}
    if source_trace_report_path and mvp_trace_report_path:
        source = read_json_path(source_trace_report_path, {})
        mvp = read_json_path(mvp_trace_report_path, {})
        diff = compare_trace_reports(
            source if isinstance(source, dict) else {},
            mvp if isinstance(mvp, dict) else {},
        )
        diff["source_trace_report_path"] = source_trace_report_path
        diff["mvp_trace_report_path"] = mvp_trace_report_path
        return diff
    return {
        "schema_version": "1",
        "status": "missing_trace_diff",
        "required_next_trace": "Generate source and MVP runtime-semantics-trace-report.json and compare them with eval-trace.py.",
        "partial_gate_ids": [],
        "passed_gate_ids": [],
        "failed_gate_ids": sorted(REQUIRED_GATE_IDS),
        "intentionally_different_gate_ids": [],
        "gate_results": [
            {
                "gate_id": gate_id,
                "status": "fail",
                "reason": "normalized_trace_diff_missing",
                "source_event_count": 0,
                "mvp_event_count": 0,
            }
            for gate_id in sorted(REQUIRED_GATE_IDS)
        ],
    }


def trace_diff_is_usable(diff: dict[str, Any]) -> bool:
    if not isinstance(diff, dict) or not diff:
        return False
    if diff.get("status") != "compared":
        return False
    results = diff.get("gate_results", []) or []
    if not results:
        return False
    statuses = {str(item.get("status", "")) for item in results if isinstance(item, dict)}
    return bool(statuses) and statuses.issubset({"pass", "fail", "intentionally_different"})


def finalize_gate_partition(report: dict[str, Any], *, gate_level: str) -> None:
    if gate_level not in {"comparative", "release"}:
        report["gate_statuses"] = existing_gate_statuses(report)
        return
    statuses = gate_statuses_from_trace_diff(report.get("normalized_trace_diff", {}) or {})
    if not statuses:
        statuses = {
            gate_id: {
                "gate_id": gate_id,
                "status": "fail",
                "reason": "normalized_trace_diff_missing",
            }
            for gate_id in sorted(REQUIRED_GATE_IDS)
        }
    apply_report_blockers_to_gate_statuses(report, statuses, gate_level=gate_level)
    ordered = [statuses[gate_id] for gate_id in sorted(REQUIRED_GATE_IDS)]
    report["gate_statuses"] = ordered
    report["passed_gate_ids"] = [
        item["gate_id"] for item in ordered if item["status"] == "pass"
    ]
    report["failed_gate_ids"] = [
        item["gate_id"] for item in ordered if item["status"] == "fail"
    ]
    report["intentionally_different_gate_ids"] = [
        item["gate_id"]
        for item in ordered
        if item["status"] == "intentionally_different"
    ]
    report["partial_gate_ids"] = []


def existing_gate_statuses(report: dict[str, Any]) -> list[dict[str, str]]:
    statuses: list[dict[str, str]] = []
    for field, status in (
        ("passed_gate_ids", "pass"),
        ("failed_gate_ids", "fail"),
        ("intentionally_different_gate_ids", "intentionally_different"),
    ):
        for gate_id in report.get(field, []) or []:
            statuses.append({"gate_id": str(gate_id), "status": status, "reason": field})
    for gate_id in report.get("partial_gate_ids", []) or []:
        statuses.append({"gate_id": str(gate_id), "status": "partial", "reason": "partial_gate_ids"})
    return sorted(statuses, key=lambda item: item["gate_id"])


def gate_statuses_from_trace_diff(diff: dict[str, Any]) -> dict[str, dict[str, Any]]:
    statuses: dict[str, dict[str, Any]] = {}
    for item in diff.get("gate_results", []) or []:
        if not isinstance(item, dict):
            continue
        gate_id = str(item.get("gate_id", ""))
        if gate_id not in REQUIRED_GATE_IDS:
            continue
        status = str(item.get("status", ""))
        if status not in {"pass", "fail", "intentionally_different"}:
            status = "fail"
        statuses[gate_id] = {
            "gate_id": gate_id,
            "status": status,
            "reason": str(item.get("reason", "")) or "trace_diff",
            "source_event_count": item.get("source_event_count", 0),
            "mvp_event_count": item.get("mvp_event_count", 0),
        }
    for gate_id in REQUIRED_GATE_IDS:
        statuses.setdefault(
            gate_id,
            {
                "gate_id": gate_id,
                "status": "fail",
                "reason": "gate_missing_from_trace_diff",
            },
        )
    return statuses


def apply_report_blockers_to_gate_statuses(
    report: dict[str, Any],
    statuses: dict[str, dict[str, Any]],
    *,
    gate_level: str,
) -> None:
    if int_or_none(report.get("failure_kind_blank_count")) not in {None, 0}:
        force_gate_failure(statuses, "G-S14", "failure_kind_blank_count_nonzero")
    diff = report.get("normalized_trace_diff", {}) or {}
    if diff.get("status") != "compared":
        for gate_id in REQUIRED_GATE_IDS:
            force_gate_failure(statuses, gate_id, str(diff.get("status") or "trace_diff_missing"))
    if gate_level == "release":
        uat = report.get("uat_equivalent", {}) or {}
        results = uat.get("evidence_results", {}) or {}
        browser = results.get("browser_readiness", {}) or {}
        interaction = results.get("interaction", {}) or {}
        tui = results.get("tui", {}) or {}
        if evidence_not_pass(browser) or evidence_not_pass(interaction):
            force_gate_failure(statuses, "G-S12", "release_browser_or_interaction_evidence_not_pass")
        if evidence_not_pass(tui):
            force_gate_failure(statuses, "G-S16", "release_tui_evidence_not_pass")


def force_gate_failure(
    statuses: dict[str, dict[str, Any]],
    gate_id: str,
    reason: str,
) -> None:
    current = statuses.get(gate_id, {"gate_id": gate_id})
    current["status"] = "fail"
    reasons = [part for part in str(current.get("reason", "")).split(";") if part]
    if reason not in reasons:
        reasons.append(reason)
    current["reason"] = ";".join(reasons)
    statuses[gate_id] = current


def evidence_not_pass(value: dict[str, Any]) -> bool:
    return str(value.get("status", "")) != "pass"


def read_json_path(path: str, default: Any) -> Any:
    try:
        return json.loads(Path(path).read_text(encoding="utf-8"))
    except Exception:
        return default


def compare_mvp_to_anvildev(
    *,
    mvp_summary_path: str,
    anvildev_summary_path: str,
    intentional_difference_evidence_paths: list[str],
    warn_delta_pp: float = DEFAULT_WARN_DELTA_PP,
    fail_delta_pp: float = DEFAULT_FAIL_DELTA_PP,
) -> dict[str, Any]:
    mvp = summary_snapshot(mvp_summary_path)
    source = summary_snapshot(anvildev_summary_path)
    delta = round(mvp["success_rate"] - source["success_rate"], 2)
    threshold_status = "pass"
    if delta <= -fail_delta_pp:
        threshold_status = "intentional_difference" if intentional_difference_evidence_paths else "fail"
    elif delta <= -warn_delta_pp:
        threshold_status = "warn"
    false_positive_delta = (
        mvp["acceptance_false_positive_count"]
        - source["acceptance_false_positive_count"]
    )
    stage_regressions = regressions(
        mvp["failure_layer_counts"],
        source["failure_layer_counts"],
    )
    failure_kind_regressions = regressions(
        mvp["failure_kind_counts"],
        source["failure_kind_counts"],
    )
    acceptance_delta = none_delta(
        mvp["acceptance_success_rate"],
        source["acceptance_success_rate"],
    )
    correct_failure_detection = (
        delta < 0
        and false_positive_delta < 0
        and (acceptance_delta is None or acceptance_delta >= 0)
    )
    return {
        "status": "compared",
        "mvp_summary_path": mvp_summary_path,
        "anvildev_summary_path": anvildev_summary_path,
        "mvp": mvp,
        "anvildev": source,
        "success_rate_delta_pp": delta,
        "acceptance_success_rate_delta_pp": acceptance_delta,
        "acceptance_false_positive_delta": false_positive_delta,
        "stage_regressions": stage_regressions,
        "failure_kind_regressions": failure_kind_regressions,
        "correct_failure_detection": correct_failure_detection,
        "threshold": {
            "warn_delta_pp": warn_delta_pp,
            "fail_delta_pp": fail_delta_pp,
            "status": threshold_status,
        },
        "intentional_difference_evidence_paths": intentional_difference_evidence_paths,
    }


def summary_snapshot(path: str) -> dict[str, Any]:
    rows = read_summary_path(path)
    return {
        "summary_path": path,
        "total": len(rows),
        "success": count_true(rows, "success"),
        "success_rate": percent(count_true(rows, "success"), len(rows)),
        "acceptance_success_rate": scoped_rate(rows, "acceptance_success"),
        "acceptance_false_positive_count": count_true(rows, "acceptance_false_positive"),
        "failure_kind_counts": counts(
            normalize_blank(row.get("failure_kind"))
            for row in rows
            if row.get("success") not in {"true", "diagnostic_skipped"}
        ),
        "failure_layer_counts": counts(
            normalize_blank(row.get("failure_layer"))
            for row in rows
            if row.get("success") not in {"true", "diagnostic_skipped"}
        ),
        "release_gate_status_counts": counts(
            normalize_blank(row.get("release_gate_status")) for row in rows
        ),
        "blank_failure_kind_count": blank_failure_kind_count(rows),
    }


def release_gate_evidence(
    *,
    existing: dict[str, Any],
    gate_level: str,
    uat_evidence_paths: list[str],
    browser_evidence_paths: list[str],
    interaction_evidence_paths: list[str],
    tui_event_paths: list[str],
) -> dict[str, Any]:
    evidence = dict(existing)
    evidence["evidence_paths"] = merge_paths(
        list(evidence.get("evidence_paths", []) or []), uat_evidence_paths
    )
    evidence["browser_readiness_evidence_paths"] = merge_paths(
        list(evidence.get("browser_readiness_evidence_paths", []) or []),
        browser_evidence_paths,
    )
    evidence["interaction_evidence_paths"] = merge_paths(
        list(evidence.get("interaction_evidence_paths", []) or []),
        interaction_evidence_paths,
    )
    evidence["tui_run_event_paths"] = merge_paths(
        list(evidence.get("tui_run_event_paths", []) or []), tui_event_paths
    )
    evidence["evidence_results"] = release_evidence_results(evidence)
    gaps = release_evidence_gaps(evidence)
    blockers = release_evidence_blockers(evidence)
    if gate_level == "release":
        if blockers:
            evidence["status"] = "fail"
        else:
            evidence["status"] = "pass" if not gaps else "partial"
        evidence["missing_required_evidence"] = gaps
    else:
        if blockers:
            evidence["status"] = "fail"
        else:
            evidence.setdefault("status", "partial" if gaps else "pass")
    if blockers:
        evidence["reason"] = "Release evidence content failed: " + "; ".join(blockers)
    elif gaps:
        evidence["reason"] = "Release gate full pass requires browser readiness, interaction, and TUI run event evidence."
    return evidence


def release_evidence_gaps(uat: dict[str, Any]) -> list[str]:
    missing = []
    if not (uat.get("evidence_paths") or []):
        missing.append("uat_evidence_paths")
    if not (uat.get("browser_readiness_evidence_paths") or []):
        missing.append("browser_readiness_evidence_paths")
    if not (uat.get("interaction_evidence_paths") or []):
        missing.append("interaction_evidence_paths")
    if not (uat.get("tui_run_event_paths") or []):
        missing.append("tui_run_event_paths")
    results = uat.get("evidence_results", {}) or {}
    for key, value in results.items():
        if not isinstance(value, dict):
            continue
        if value.get("status") in {"missing", "partial"}:
            missing.append(f"{key}:{value.get('reason') or value.get('failure_kind') or value.get('status')}")
    return missing


def release_evidence_blockers(uat: dict[str, Any]) -> list[str]:
    blockers: list[str] = []
    results = uat.get("evidence_results", {}) or {}
    for key, value in results.items():
        if not isinstance(value, dict):
            continue
        if value.get("status") in {"fail", "invalid"}:
            reason = value.get("failure_kind") or value.get("reason") or value.get("status")
            blockers.append(f"{key}:{reason}")
    return blockers


def release_evidence_results(evidence: dict[str, Any]) -> dict[str, dict[str, Any]]:
    return {
        "uat": evaluate_text_evidence(evidence.get("evidence_paths", []) or []),
        "browser_readiness": evaluate_browser_evidence(
            evidence.get("browser_readiness_evidence_paths", []) or []
        ),
        "interaction": evaluate_interaction_evidence(
            evidence.get("interaction_evidence_paths", []) or []
        ),
        "tui": evaluate_tui_evidence(evidence.get("tui_run_event_paths", []) or []),
    }


def evaluate_text_evidence(paths: list[str]) -> dict[str, Any]:
    if not paths:
        return {"status": "missing", "reason": "uat_evidence_missing"}
    for raw in paths:
        path = Path(raw)
        if not path.is_file():
            return {"status": "invalid", "path": raw, "reason": "evidence_missing"}
        try:
            text = path.read_text(encoding="utf-8", errors="replace")
        except Exception as err:
            return {
                "status": "invalid",
                "path": raw,
                "reason": "evidence_unreadable",
                "error": type(err).__name__,
            }
        if text.strip():
            return {"status": "pass", "path": raw}
    return {"status": "invalid", "path": paths[0], "reason": "evidence_empty"}


def evaluate_browser_evidence(paths: list[str]) -> dict[str, Any]:
    parsed = first_json_evidence(paths)
    if parsed["status"] != "loaded":
        return parsed
    value = parsed["value"]
    path = parsed["path"]
    ok = bool_field(value, "ok", "success", "browser_success", "http_ok")
    status_code = int_field(value, "http_status", "status", "status_code")
    details = value.get("browser_details") if isinstance(value.get("browser_details"), dict) else {}
    if status_code is None:
        status_code = int_field(details, "http_status", "status", "status_code")
    if status_code is not None and status_code >= 400:
        failure = browser_failure_kind(value, details, status_code)
        return {
            "status": "fail",
            "path": path,
            "failure_kind": failure,
            "http_status": status_code,
            "reason": failure,
        }
    text_status = text_field(value, "status") or text_field(details, "status")
    if browser_unavailable_status(text_status):
        reason = browser_unavailable_reason(value, details, text_status or "unavailable")
        return {
            "status": "partial",
            "path": path,
            "reason": reason,
            "http_status": status_code or "",
        }
    if ok is False:
        failure = browser_failure_kind(value, details, status_code)
        return {
            "status": "fail",
            "path": path,
            "failure_kind": failure,
            "http_status": status_code or "",
            "reason": failure,
        }
    explicit_failure = browser_readiness_detail_failure(value, details)
    if explicit_failure:
        return {
            "status": "fail",
            "path": path,
            "failure_kind": explicit_failure,
            "http_status": status_code or "",
            "reason": explicit_failure,
        }
    if browser_readiness_has_required_detail(value, details):
        return {"status": "pass", "path": path, "http_status": status_code or ""}
    if ok is True or (status_code is not None and 200 <= status_code < 400) or text_status in {"ok", "pass", "passed", "ready"}:
        return {
            "status": "partial",
            "path": path,
            "reason": "browser_render_evidence_missing",
            "http_status": status_code or "",
        }
    return {
        "status": "invalid",
        "path": path,
        "reason": "evidence_missing_required_key",
    }


def evaluate_interaction_evidence(paths: list[str]) -> dict[str, Any]:
    parsed = first_json_evidence(paths)
    if parsed["status"] != "loaded":
        return parsed
    value = parsed["value"]
    path = parsed["path"]
    ok = bool_field(value, "ok", "success", "interaction_success")
    details = value.get("browser_details") if isinstance(value.get("browser_details"), dict) else {}
    explicit_failure = interaction_detail_failure(value, details)
    if explicit_failure:
        return {
            "status": "fail",
            "path": path,
            "failure_kind": explicit_failure,
            "reason": explicit_failure,
        }
    if ok is True:
        if interaction_has_required_detail(value, details):
            return {"status": "pass", "path": path}
        return {
            "status": "partial",
            "path": path,
            "reason": "interaction_detail_missing",
        }
    if ok is False:
        reason = text_field(value, "reason")
        if reason in {"canvas_not_available", "canvas_unavailable"}:
            failure_kind = "canvas_unavailable"
        elif reason in {"interactive_surface_missing", "interaction_surface_missing"}:
            failure_kind = "interactive_surface_missing"
        else:
            failure_kind = text_field(value, "failure_kind", "error_kind") or "interaction_failed"
        return {
            "status": "fail",
            "path": path,
            "failure_kind": failure_kind,
            "reason": reason or "interaction_failed",
        }
    if value.get("skipped") is True:
        return {
            "status": "partial",
            "path": path,
            "reason": text_field(value, "reason") or "interaction_skipped",
        }
    return {
        "status": "invalid",
        "path": path,
        "reason": "evidence_missing_required_key",
    }


def evaluate_tui_evidence(paths: list[str]) -> dict[str, Any]:
    if not paths:
        return {"status": "missing", "reason": "tui_run_event_paths_missing"}
    for raw in paths:
        path = Path(raw)
        if not path.is_file():
            return {"status": "invalid", "path": raw, "reason": "evidence_missing"}
        try:
            events = read_jsonl(path)
        except ValueError as err:
            return {
                "status": "invalid",
                "path": raw,
                "reason": "evidence_invalid",
                "error": str(err),
            }
        if not events:
            return {
                "status": "fail",
                "path": raw,
                "failure_kind": "silent_exit",
                "reason": "tui_events_empty",
            }
        run_stop = next((event for event in reversed(events) if event.get("event") == "run_stop"), None)
        if run_stop is None:
            return {
                "status": "fail",
                "path": raw,
                "failure_kind": "silent_exit",
                "reason": "run_stop_missing",
            }
        if run_stop.get("ok") is not True:
            return {
                "status": "fail",
                "path": raw,
                "failure_kind": run_stop.get("failure_kind") or "process_failure",
                "reason": run_stop.get("stop_reason") or run_stop.get("failure_kind") or "process_failure",
            }
        if not run_stop.get("stop_reason"):
            return {
                "status": "fail",
                "path": raw,
                "failure_kind": "silent_exit",
                "reason": "run_stop_reason_missing",
            }
        stop = next((event for event in reversed(events) if event.get("event") == "tui_command_stop"), None)
        if stop is None:
            return {
                "status": "fail",
                "path": raw,
                "failure_kind": "silent_exit",
                "reason": "tui_command_stop_missing",
            }
        if stop.get("ok") is True:
            return {"status": "pass", "path": raw, "stop_reason": run_stop.get("stop_reason")}
        return {
            "status": "fail",
            "path": raw,
            "failure_kind": stop.get("failure_kind") or "tui_command_failed",
            "reason": stop.get("primary_reason") or stop.get("failure_kind") or "tui_command_failed",
        }
    return {"status": "missing", "reason": "tui_run_event_paths_missing"}


def first_json_evidence(paths: list[str]) -> dict[str, Any]:
    if not paths:
        return {"status": "missing", "reason": "evidence_missing"}
    for raw in paths:
        path = Path(raw)
        if not path.is_file():
            return {"status": "invalid", "path": raw, "reason": "evidence_missing"}
        try:
            parsed = json.loads(path.read_text(encoding="utf-8"))
        except Exception as err:
            return {
                "status": "invalid",
                "path": raw,
                "reason": "evidence_invalid",
                "error": type(err).__name__,
            }
        if not isinstance(parsed, dict):
            return {"status": "invalid", "path": raw, "reason": "evidence_invalid"}
        return {"status": "loaded", "path": raw, "value": parsed}
    return {"status": "missing", "reason": "evidence_missing"}


def read_jsonl(path: Path) -> list[dict[str, Any]]:
    events: list[dict[str, Any]] = []
    for line_number, line in enumerate(path.read_text(encoding="utf-8").splitlines(), start=1):
        if not line.strip():
            continue
        try:
            event = json.loads(line)
        except json.JSONDecodeError as err:
            raise ValueError(f"line {line_number}: {err.msg}") from err
        if not isinstance(event, dict):
            raise ValueError(f"line {line_number}: event must be an object")
        events.append(event)
    return events


def browser_failure_kind(
    value: dict[str, Any],
    details: dict[str, Any],
    status_code: int | None,
) -> str:
    kind = (
        text_field(value, "browser_failure_kind", "failure_kind", "error_kind")
        or text_field(details, "browser_failure_kind", "failure_kind", "error_kind")
    )
    if kind in {"tailwind_dev_pipeline_failure", "css_dev_pipeline_failure", "nextjs_dev_pipeline_failure"}:
        return kind
    if status_code is not None and status_code >= 400:
        return f"browser_http_{status_code}"
    return kind or "browser_failed"


def browser_unavailable_status(status: str) -> bool:
    return status in {"not_enabled", "adapter_not_implemented", "unavailable", "skipped"} or status.startswith(
        ("unavailable:", "browser_unavailable:")
    ) or status == "browser_unavailable"


def browser_unavailable_reason(value: dict[str, Any], details: dict[str, Any], status: str) -> str:
    return (
        text_field(value, "browser_failure_kind", "failure_kind", "error_kind", "reason")
        or text_field(details, "browser_failure_kind", "failure_kind", "error_kind", "reason")
        or status
    )


def browser_readiness_has_required_detail(value: dict[str, Any], details: dict[str, Any]) -> bool:
    return any_true(value, details, "route_rendered", "rendered", "page_loaded", "dom_ready")


def browser_readiness_detail_failure(value: dict[str, Any], details: dict[str, Any]) -> str:
    if any_false(value, details, "route_rendered", "rendered", "page_loaded", "dom_ready"):
        return "browser_route_not_rendered"
    return ""


def interaction_has_required_detail(value: dict[str, Any], details: dict[str, Any]) -> bool:
    return any_true(
        value,
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


def interaction_detail_failure(value: dict[str, Any], details: dict[str, Any]) -> str:
    if any_false(value, details, "canvas_found", "canvas_available"):
        return "canvas_unavailable"
    if any_false(value, details, "interactive_surface", "interaction_surface"):
        return "interactive_surface_missing"
    if any_false(
        value,
        details,
        "input_event_observed",
        "keyboard_event_observed",
        "pointer_event_observed",
    ):
        return "input_event_missing"
    if any_false(value, details, "state_changed", "visible_state_changed"):
        return "interaction_state_change_missing"
    return ""


def any_true(value: dict[str, Any], details: dict[str, Any], *keys: str) -> bool:
    return any(bool_field(value, key) is True or bool_field(details, key) is True for key in keys)


def any_false(value: dict[str, Any], details: dict[str, Any], *keys: str) -> bool:
    return any(bool_field(value, key) is False or bool_field(details, key) is False for key in keys)


def bool_field(value: dict[str, Any], *keys: str) -> bool | None:
    for key in keys:
        raw = value.get(key)
        if isinstance(raw, bool):
            return raw
    return None


def int_field(value: dict[str, Any], *keys: str) -> int | None:
    for key in keys:
        raw = value.get(key)
        if isinstance(raw, bool):
            continue
        if isinstance(raw, int):
            return raw
        if isinstance(raw, str):
            try:
                return int(raw)
            except ValueError:
                continue
    return None


def text_field(value: dict[str, Any], *keys: str) -> str:
    for key in keys:
        raw = value.get(key)
        if isinstance(raw, str):
            return raw.strip().lower()
    return ""


def int_or_none(value: Any) -> int | None:
    try:
        return int(value)
    except (TypeError, ValueError):
        return None


def read_summary_path(path: str) -> list[dict[str, str]]:
    return read_summary(Path(path))


def count_true(rows: list[dict[str, str]], field: str) -> int:
    return sum(1 for row in rows if str(row.get(field, "")).lower() == "true")


def scoped_rate(rows: list[dict[str, str]], field: str) -> float | None:
    scoped = [row for row in rows if row.get(field) not in {"", None}]
    if not scoped:
        return None
    return percent(count_true(scoped, field), len(scoped))


def percent(count: int, total: int) -> float:
    if total <= 0:
        return 0.0
    return round(100.0 * count / total, 2)


def counts(values: Any) -> dict[str, int]:
    result: dict[str, int] = {}
    for value in values:
        key = normalize_blank(value)
        if not key:
            continue
        result[key] = result.get(key, 0) + 1
    return dict(sorted(result.items()))


def regressions(current: dict[str, int], baseline: dict[str, int]) -> list[dict[str, Any]]:
    rows = []
    for key, value in sorted(current.items()):
        base = baseline.get(key, 0)
        if value > base:
            rows.append({"key": key, "mvp": value, "anvildev": base, "delta": value - base})
    return rows


def normalize_blank(value: Any) -> str:
    text = str(value or "").strip()
    return text


def none_delta(current: float | None, baseline: float | None) -> float | None:
    if current is None or baseline is None:
        return None
    return round(current - baseline, 2)


def merge_paths(left: list[str], right: list[str]) -> list[str]:
    merged = []
    for value in [*left, *right]:
        text = str(value).strip()
        if text and text not in merged:
            merged.append(text)
    return merged


def append_unique(values: list[str], value: str) -> None:
    if value not in values:
        values.append(value)
