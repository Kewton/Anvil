from __future__ import annotations

import json
import shlex
from collections import Counter
from datetime import datetime, timezone
from pathlib import Path
from typing import Any

from .artifacts import write_json, write_jsonl
from .redaction import redact_text
from .run_summary import read_summary


TRACE_REPORT_VERSION = "1"

STAGE_TO_GATE_IDS: dict[str, list[str]] = {
    "request_understood": ["G-S01"],
    "contract_loaded": ["G-S02"],
    "plan_generated": ["G-S03"],
    "plan_linted": ["G-S03", "G-S08"],
    "phase_started": ["G-S04"],
    "phase_context_attached": ["G-S05"],
    "step_prompt_built": ["G-S06"],
    "tool_requested": ["G-S07"],
    "tool_executed": ["G-S07"],
    "dependency_boundary_checked": ["G-S09"],
    "dependency_setup_attempted": ["G-S09"],
    "verify_started": ["G-S08"],
    "verify_failed": ["G-S08"],
    "repair_target_classified": ["G-S10"],
    "repair_attempted": ["G-S10"],
    "repair_exhausted": ["G-S10"],
    "scaffold_continuation_required": ["G-S11"],
    "recovery_handoff_saved": ["G-S13"],
    "acceptance_started": ["G-S12"],
    "acceptance_failed": ["G-S12"],
    "acceptance_passed": ["G-S12"],
    "diagnostic_emitted": ["G-S14"],
    "provider_observed": ["G-S15"],
    "manual_tui_trace_recorded": ["G-S16"],
}

SOURCE_EVENT_STAGE: dict[str, str] = {
    "task_contract_created": "contract_loaded",
    "task_contract_updated": "contract_loaded",
    "task_contract_completion": "acceptance_started",
    "verifier_started": "verify_started",
    "verifier_failed": "verify_failed",
    "verifier_repair_targeted": "repair_target_classified",
    "repair_lifecycle_started": "repair_attempted",
    "repair_lifecycle_exhausted": "repair_exhausted",
    "scaffold_pipeline_started": "scaffold_continuation_required",
    "scaffold_pipeline_recovered": "repair_attempted",
    "project_probe": "dependency_boundary_checked",
    "project_verifier": "verify_started",
}

MVP_EVENT_STAGE: dict[str, str] = {
    "acceptance_summary": "acceptance_started",
    "completion_verify": "contract_loaded",
    "dependency_build_lifecycle": "dependency_boundary_checked",
    "diagnostic_skipped": "diagnostic_emitted",
    "fallback_decision": "scaffold_continuation_required",
    "plan_capability_contract_evaluated": "contract_loaded",
    "plan_run_missed_predictive_signal": "diagnostic_emitted",
    "plan_run_readiness_evaluated": "plan_linted",
    "plan_score": "plan_generated",
    "plan_verify_coverage_evaluated": "verify_started",
    "planner_error": "plan_linted",
    "planner_quality_issue": "plan_linted",
    "planner_quality_retry": "plan_linted",
    "planner_raw_output_shape": "plan_generated",
    "postcheck_summary": "acceptance_started",
    "provider_error": "provider_observed",
    "provider_probe": "provider_observed",
    "provider_response": "provider_observed",
    "recovery_prompt_saved": "recovery_handoff_saved",
    "run_start": "request_understood",
    "scheduler_diagnostics": "diagnostic_emitted",
    "step_prompt_built": "step_prompt_built",
    "step_verify_failure": "verify_failed",
    "step_verify_repair": "repair_attempted",
    "tool_call_raw": "tool_requested",
    "tool_args_recovered": "tool_requested",
    "tool_execute": "tool_executed",
    "tool_validation_error": "tool_requested",
    "ultra_context_initialized": "phase_context_attached",
    "ultra_phase_complete": "acceptance_passed",
    "ultra_phase_context_attached": "phase_context_attached",
    "ultra_phase_execute_complete": "tool_executed",
    "ultra_phase_failed": "verify_failed",
    "ultra_phase_plan_validated": "plan_linted",
    "ultra_phase_profile_check": "verify_started",
    "ultra_phase_scaffold_complete": "scaffold_continuation_required",
    "ultra_phase_start": "phase_started",
    "ultra_plan_generation_attempt": "plan_generated",
    "ultra_plan_generation_failed": "plan_linted",
    "ultra_plan_generation_metadata_normalized": "plan_generated",
    "ultra_plan_generation_retry": "plan_linted",
    "ultra_plan_generation_tool_call_rejected": "plan_linted",
    "verify_repair_progress": "repair_attempted",
    "verify_repair_turn": "repair_attempted",
}


def write_trace_artifacts(
    run_root: Path,
    *,
    subject: str,
    binary_kind: str,
    binary_path: str,
    label: str = "",
    commit_sha: str = "",
    output_dir: Path | None = None,
) -> dict[str, Any]:
    output = output_dir or run_root
    report = build_trace_report(
        run_root,
        subject=subject,
        binary_kind=binary_kind,
        binary_path=binary_path,
        label=label,
        commit_sha=commit_sha,
        output_dir=output,
    )
    normalized_path = output / "runtime-semantics-normalized-events.jsonl"
    report_path = output / "runtime-semantics-trace-report.json"
    manifest_path = output / "runtime-semantics-trace-manifest.md"
    write_jsonl(normalized_path, report["normalized_events"])
    serializable = {key: value for key, value in report.items() if key != "normalized_events"}
    serializable["normalized_event_sequence_path"] = str(normalized_path)
    write_json(report_path, serializable)
    manifest_path.write_text(render_trace_manifest(serializable), encoding="utf-8")
    serializable["report_path"] = str(report_path)
    serializable["manifest_path"] = str(manifest_path)
    return serializable


def build_trace_report(
    run_root: Path,
    *,
    subject: str,
    binary_kind: str,
    binary_path: str,
    label: str = "",
    commit_sha: str = "",
    output_dir: Path | None = None,
) -> dict[str, Any]:
    summary_path = run_root / "summary.eval.tsv"
    rows = read_summary(summary_path) if summary_path.exists() else []
    matrix_path = run_root / "matrix.json"
    matrix = read_json(matrix_path, default=[])
    matrix_by_run = {
        str(item.get("run_id", "")): item for item in matrix if isinstance(item, dict)
    }
    normalized: list[dict[str, Any]] = []
    for row in rows:
        run_id = str(row.get("run_id", ""))
        run_dir = run_root / "runs" / run_id
        events = load_run_events(run_root, row)
        normalized.extend(normalize_events(events, row=row, subject=subject))
        silent = silent_exit_event(row=row, run_dir=run_dir, events=events, subject=subject)
        if silent:
            normalized.append(silent)
        normalized.extend(summary_diagnostic_events(row=row, subject=subject))

    stage_counts = Counter(event["stage"] for event in normalized)
    gate_counts: Counter[str] = Counter()
    for event in normalized:
        for gate_id in event.get("gate_ids", []):
            gate_counts[str(gate_id)] += 1
    manifest_rows = build_manifest_rows(run_root, rows, matrix_by_run)
    return {
        "schema_version": TRACE_REPORT_VERSION,
        "generated_at": datetime.now(timezone.utc).isoformat(),
        "trace_id": label or f"{subject}-{run_root.name}",
        "subject": subject,
        "binary_kind": binary_kind,
        "binary_path": redact_text(binary_path),
        "commit_sha": commit_sha,
        "run_root": str(run_root),
        "summary_path": str(summary_path) if summary_path.exists() else "",
        "events_path": str(run_root / "events.jsonl") if (run_root / "events.jsonl").exists() else "",
        "matrix_path": str(matrix_path) if matrix_path.exists() else "",
        "normalized_events": normalized,
        "normalized_event_count": len(normalized),
        "run_count": len(rows),
        "stage_counts": dict(sorted(stage_counts.items())),
        "gate_counts": dict(sorted(gate_counts.items())),
        "silent_exit_count": sum(
            1 for event in normalized if event.get("failure_kind") == "silent_exit_without_events"
        ),
        "known_gaps": known_gaps(rows, normalized, subject),
        "manifest_rows": manifest_rows,
        "env_redaction_status": "redacted",
        "output_dir": str(output_dir or run_root),
    }


def load_run_events(run_root: Path, row: dict[str, Any]) -> list[dict[str, Any]]:
    run_id = str(row.get("run_id", ""))
    per_run = run_root / "runs" / run_id / "anvil-events.jsonl"
    events = read_jsonl(per_run)
    if events:
        return events
    aggregate = read_jsonl(run_root / "events.jsonl")
    return [event for event in aggregate if str(event.get("run_id", "")) == run_id]


def normalize_events(
    events: list[dict[str, Any]],
    *,
    row: dict[str, Any] | None = None,
    subject: str = "",
) -> list[dict[str, Any]]:
    out: list[dict[str, Any]] = []
    for index, event in enumerate(events):
        stage = stage_for_event(event)
        if not stage:
            continue
        normalized = {
            "schema_version": TRACE_REPORT_VERSION,
            "subject": subject,
            "run_id": str(event.get("run_id") or (row or {}).get("run_id", "")),
            "mode": str(event.get("mode") or (row or {}).get("mode", "")),
            "scenario": str((row or {}).get("scenario", "")),
            "event_index": index,
            "source_event": str(event.get("event", "")),
            "stage": stage,
            "gate_ids": STAGE_TO_GATE_IDS.get(stage, []),
        }
        failure_kind = failure_kind_from_event(event, stage)
        if failure_kind:
            normalized["failure_kind"] = failure_kind
        phase_id = event.get("phase_id")
        if phase_id not in {"", None}:
            normalized["phase_id"] = str(phase_id)
        normalized.update(safe_event_details(event))
        out.append(normalized)
        manual_event = manual_tui_trace_event(event, normalized)
        if manual_event:
            out.append(manual_event)
    return out


def manual_tui_trace_event(
    event: dict[str, Any],
    normalized: dict[str, Any],
) -> dict[str, Any] | None:
    if event.get("event") != "run_start":
        return None
    if event_bool(event.get("eval_events_override")):
        return None
    manual = dict(normalized)
    manual["source_event"] = "run_start"
    manual["stage"] = "manual_tui_trace_recorded"
    manual["gate_ids"] = STAGE_TO_GATE_IDS["manual_tui_trace_recorded"]
    manual.pop("failure_kind", None)
    return manual


def stage_for_event(event: dict[str, Any]) -> str:
    name = str(event.get("event", ""))
    if name in MVP_EVENT_STAGE:
        stage = MVP_EVENT_STAGE[name]
    elif name in SOURCE_EVENT_STAGE:
        stage = SOURCE_EVENT_STAGE[name]
    else:
        stage = ""
    if name == "acceptance_summary":
        if event_bool(event.get("acceptance_success")):
            return "acceptance_passed"
        if event_false(event.get("acceptance_success")):
            return "acceptance_failed"
    if name == "postcheck_summary":
        return "acceptance_passed" if event_bool(event.get("ok")) else "acceptance_failed"
    if name == "completion_verify":
        if event_false(event.get("runtime_acceptance_passed")):
            return "acceptance_failed"
        if event_bool(event.get("runtime_acceptance_passed")):
            return "acceptance_passed"
    if name == "dependency_build_lifecycle":
        if event_bool(event.get("setup_attempted")):
            return "dependency_setup_attempted"
        lifecycle = " ".join(str(item) for item in event.get("lifecycle_stages", []) or [])
        if "setup_attempted" in lifecycle or "setup_passed" in lifecycle:
            return "dependency_setup_attempted"
    if name == "loop_stop":
        return loop_stop_stage(event)
    if name == "tool_execute":
        return "tool_executed"
    if name == "tool_validation_error":
        return "tool_requested"
    if name == "tool_args_recovered":
        return "tool_requested"
    return stage


def loop_stop_stage(event: dict[str, Any]) -> str:
    reason = " ".join(
        str(event.get(key, ""))
        for key in ("reason", "primary_reason", "last_blocking_reason")
        if event.get(key) not in {"", None}
    ).lower()
    if "repair" in reason and ("exhaust" in reason or "max" in reason):
        return "repair_exhausted"
    if "missing" in reason or "artifact" in reason or "capability" in reason:
        return "acceptance_failed"
    if "dependency" in reason or "build" in reason:
        return "dependency_boundary_checked"
    if "tool" in reason:
        return "tool_requested"
    return "diagnostic_emitted"


def failure_kind_from_event(event: dict[str, Any], stage: str) -> str:
    for key in (
        "failure_kind",
        "planner_error_kind",
        "acceptance_failure_kind",
        "plan_output_failure_kind",
        "error_kind",
        "reason_kind",
    ):
        value = event.get(key)
        if value not in {"", None}:
            return str(value)
    if stage == "verify_failed":
        return "verify_failed"
    if stage == "acceptance_failed":
        return "acceptance_failed"
    if stage == "repair_exhausted":
        return "repair_exhausted"
    return ""


def safe_event_details(event: dict[str, Any]) -> dict[str, Any]:
    allowed_keys = {
        "argument_shape",
        "command_kind",
        "status",
        "planner_stage",
        "repair_target",
        "runtime_acceptance_primary_reason",
        "profile",
        "provider",
        "model",
        "planner_provider",
        "planner_model",
        "http_status",
    }
    out: dict[str, Any] = {}
    for key in allowed_keys:
        if key in event and present(event.get(key)):
            out[key] = redact_value(event[key])
    return out


def silent_exit_event(
    *,
    row: dict[str, Any],
    run_dir: Path,
    events: list[dict[str, Any]],
    subject: str,
) -> dict[str, Any] | None:
    if events:
        return None
    if not is_failed_row(row):
        return None
    return {
        "schema_version": TRACE_REPORT_VERSION,
        "subject": subject,
        "run_id": str(row.get("run_id", "")),
        "mode": str(row.get("mode", "")),
        "scenario": str(row.get("scenario", "")),
        "event_index": -1,
        "source_event": "missing_run_events",
        "stage": "diagnostic_emitted",
        "gate_ids": ["G-S14", "G-S16"],
        "failure_kind": "silent_exit_without_events",
        "rc": str(row.get("rc", "")),
        "run_dir": str(run_dir),
    }


def summary_diagnostic_events(row: dict[str, Any], *, subject: str) -> list[dict[str, Any]]:
    if not is_failed_row(row):
        return []
    failure_kind = str(row.get("failure_kind") or row.get("acceptance_failure_kind") or "")
    if not failure_kind:
        failure_kind = "summary_failure_without_kind"
    return [
        {
            "schema_version": TRACE_REPORT_VERSION,
            "subject": subject,
            "run_id": str(row.get("run_id", "")),
            "mode": str(row.get("mode", "")),
            "scenario": str(row.get("scenario", "")),
            "event_index": -2,
            "source_event": "summary_failure",
            "stage": "diagnostic_emitted",
            "gate_ids": ["G-S14"],
            "failure_kind": failure_kind,
            "failure_layer": str(row.get("failure_layer", "")),
        }
    ]


def is_failed_row(row: dict[str, Any]) -> bool:
    if str(row.get("success", "")).lower() == "diagnostic_skipped":
        return False
    if str(row.get("success", "")).lower() == "false":
        return True
    if str(row.get("rc", "")).strip() not in {"", "0"}:
        return True
    if str(row.get("acceptance_success", "")).lower() == "false":
        return True
    if str(row.get("postcheck_failure", "")).lower() == "true":
        return True
    return False


def build_manifest_rows(
    run_root: Path,
    rows: list[dict[str, Any]],
    matrix_by_run: dict[str, dict[str, Any]],
) -> list[dict[str, Any]]:
    out: list[dict[str, Any]] = []
    for row in rows:
        run_id = str(row.get("run_id", ""))
        spec = matrix_by_run.get(run_id, {})
        run_dir = run_root / "runs" / run_id
        command = redacted_command(run_dir / "command.txt")
        out.append(
            {
                "run_id": run_id,
                "suite": str(row.get("suite", "")),
                "scenario": str(row.get("scenario", "")),
                "mode": str(row.get("mode", "")),
                "success": str(row.get("success", "")),
                "failure_kind": str(row.get("failure_kind", "")),
                "command": command,
                "provider_model_pair": provider_model_pair(row, spec),
                "events_path": str(run_dir / "anvil-events.jsonl")
                if (run_dir / "anvil-events.jsonl").exists()
                else "",
                "summary_path": str(run_root / "summary.eval.tsv"),
            }
        )
    return out


def provider_model_pair(row: dict[str, Any], spec: dict[str, Any]) -> str:
    main = spec.get("main", {}) if isinstance(spec, dict) else {}
    planner = spec.get("planner", {}) if isinstance(spec, dict) else {}
    main_provider = row.get("main_provider") or main.get("provider", "")
    main_model = row.get("main_model") or main.get("model", "")
    planner_provider = row.get("planner_provider") or planner.get("provider", "")
    planner_model = row.get("planner_model") or planner.get("model", "")
    return f"{main_provider}:{main_model} planner={planner_provider}:{planner_model}"


def known_gaps(rows: list[dict[str, Any]], normalized: list[dict[str, Any]], subject: str) -> list[str]:
    gaps: list[str] = []
    if subject == "mvp-anvilminimal":
        gaps.append("source same-condition trace must be compared before parity gates can pass")
    if any(event.get("failure_kind") == "silent_exit_without_events" for event in normalized):
        gaps.append("one or more failed runs exited without per-run events")
    if not any(event.get("stage") == "manual_tui_trace_recorded" for event in normalized):
        gaps.append("manual TUI trace evidence is not attached")
    if not rows:
        gaps.append("summary rows are missing")
    return gaps


def render_trace_manifest(report: dict[str, Any]) -> str:
    lines = [
        "# Runtime Semantics Trace Manifest",
        "",
        f"- trace_id: `{report.get('trace_id', '')}`",
        f"- subject: `{report.get('subject', '')}`",
        f"- binary_kind: `{report.get('binary_kind', '')}`",
        f"- binary_path: `{report.get('binary_path', '')}`",
        f"- commit_sha: `{report.get('commit_sha', '')}`",
        f"- run_root: `{report.get('run_root', '')}`",
        f"- summary_path: `{report.get('summary_path', '')}`",
        f"- events_path: `{report.get('events_path', '')}`",
        f"- normalized_event_sequence_path: `{report.get('normalized_event_sequence_path', '')}`",
        f"- env_redaction_status: `{report.get('env_redaction_status', '')}`",
        "",
        "## Stage Counts",
        "",
        "| stage | count |",
        "| --- | ---: |",
    ]
    for stage, count in (report.get("stage_counts", {}) or {}).items():
        lines.append(f"| `{stage}` | {count} |")
    lines.extend(["", "## Gate Counts", "", "| gate_id | count |", "| --- | ---: |"])
    for gate_id, count in (report.get("gate_counts", {}) or {}).items():
        lines.append(f"| `{gate_id}` | {count} |")
    lines.extend(["", "## Runs", "", "| run_id | mode | success | failure_kind | events | command |", "| --- | --- | --- | --- | --- | --- |"])
    for item in report.get("manifest_rows", []) or []:
        event_status = "yes" if item.get("events_path") else "missing"
        command = str(item.get("command", "")).replace("|", "\\|")
        lines.append(
            f"| `{item.get('run_id', '')}` | `{item.get('mode', '')}` | "
            f"`{item.get('success', '')}` | `{item.get('failure_kind', '')}` | "
            f"{event_status} | `{command}` |"
        )
    gaps = report.get("known_gaps", []) or []
    if gaps:
        lines.extend(["", "## Known Gaps", ""])
        for gap in gaps:
            lines.append(f"- {gap}")
    lines.append("")
    return "\n".join(lines)


def compare_trace_reports(source: dict[str, Any], mvp: dict[str, Any]) -> dict[str, Any]:
    source_stages = set((source.get("stage_counts", {}) or {}).keys())
    mvp_stages = set((mvp.get("stage_counts", {}) or {}).keys())
    source_gates = set((source.get("gate_counts", {}) or {}).keys())
    mvp_gates = set((mvp.get("gate_counts", {}) or {}).keys())
    return {
        "schema_version": TRACE_REPORT_VERSION,
        "source_trace_id": source.get("trace_id", ""),
        "mvp_trace_id": mvp.get("trace_id", ""),
        "missing_stages_in_mvp": sorted(source_stages - mvp_stages),
        "extra_stages_in_mvp": sorted(mvp_stages - source_stages),
        "missing_gate_ids_in_mvp": sorted(source_gates - mvp_gates),
        "extra_gate_ids_in_mvp": sorted(mvp_gates - source_gates),
    }


def redacted_command(path: Path) -> str:
    if not path.exists():
        return ""
    parts = parse_command_line(path.read_text(encoding="utf-8", errors="replace"))
    action_flags = {
        "--prompt",
        "--plan-steps",
        "--plan-run",
        "--run-plan",
        "--ultra-plan",
        "--ultra-plan-run",
        "--run-ultra-plan",
    }
    redacted: list[str] = []
    redact_next = False
    for part in parts:
        if redact_next:
            redacted.append("<redacted-task-prompt>")
            redact_next = False
            continue
        if part in action_flags:
            redacted.append(part)
            redact_next = True
            continue
        redacted.append(redact_text(part))
    return shlex.join(redacted)


def parse_command_line(text: str) -> list[str]:
    try:
        return shlex.split(text.strip())
    except ValueError:
        return [redact_text(text.strip())]


def read_json(path: Path, default: Any) -> Any:
    if not path.exists():
        return default
    try:
        return json.loads(path.read_text(encoding="utf-8"))
    except json.JSONDecodeError:
        return default


def read_jsonl(path: Path) -> list[dict[str, Any]]:
    if not path.exists():
        return []
    out: list[dict[str, Any]] = []
    for line in path.read_text(encoding="utf-8", errors="replace").splitlines():
        if not line.strip():
            continue
        try:
            value = json.loads(line)
        except json.JSONDecodeError:
            continue
        if isinstance(value, dict):
            out.append(value)
    return out


def event_bool(value: Any) -> bool:
    if isinstance(value, bool):
        return value
    return str(value).lower() == "true"


def event_false(value: Any) -> bool:
    if isinstance(value, bool):
        return not value
    return str(value).lower() == "false"


def present(value: Any) -> bool:
    return value is not None and value != ""


def redact_value(value: Any) -> Any:
    if isinstance(value, str):
        return redact_text(value)
    if isinstance(value, list):
        return [redact_value(item) for item in value]
    if isinstance(value, dict):
        return {str(key): redact_value(item) for key, item in value.items()}
    return value
