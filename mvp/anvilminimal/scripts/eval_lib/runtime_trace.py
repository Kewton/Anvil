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
REQUIRED_GATE_IDS = [f"G-S{index:02d}" for index in range(1, 17)]
FAILURE_STAGES = {
    "verify_failed",
    "repair_exhausted",
    "acceptance_failed",
    "diagnostic_emitted",
}

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
    "source_phase_context_observed": "phase_context_attached",
    "source_step_prompt_observed": "step_prompt_built",
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
    "browser_oracle_summary": "acceptance_started",
    "completion_verify": "contract_loaded",
    "dependency_build_lifecycle": "dependency_boundary_checked",
    "dev_server_lifecycle": "acceptance_started",
    "deterministic_scaffold_recovery": "scaffold_continuation_required",
    "diagnostic_skipped": "diagnostic_emitted",
    "fallback_decision": "scaffold_continuation_required",
    "final_acceptance_repair_complete": "repair_attempted",
    "final_acceptance_repair_exhausted": "repair_exhausted",
    "final_acceptance_repair_failed": "repair_attempted",
    "final_acceptance_repair_start": "repair_attempted",
    "plan_capability_contract_evaluated": "contract_loaded",
    "plan_final_contract": "contract_loaded",
    "plan_run_missed_predictive_signal": "diagnostic_emitted",
    "plan_run_readiness_evaluated": "plan_linted",
    "plan_score": "plan_generated",
    "plan_verify_coverage_evaluated": "verify_started",
    "phase_verification_result": "verify_started",
    "planner_error": "plan_linted",
    "planner_parse_error": "plan_linted",
    "planner_quality_retry_degraded": "plan_linted",
    "planner_quality_retry_exhausted": "plan_linted",
    "planner_quality_issue": "plan_linted",
    "planner_quality_retry": "plan_linted",
    "planner_quality_warning": "plan_linted",
    "planner_raw_output_shape": "plan_generated",
    "planner_verify_command_normalized": "plan_linted",
    "postcheck_summary": "acceptance_started",
    "provider_error": "provider_observed",
    "provider_probe": "provider_observed",
    "provider_request": "provider_observed",
    "provider_response": "provider_observed",
    "provider_parse_error": "provider_observed",
    "provider_retry": "provider_observed",
    "profile_auto_repair_continuation_complete": "repair_attempted",
    "profile_auto_repair_continuation_incomplete": "repair_attempted",
    "profile_auto_repair_continuation_start": "repair_attempted",
    "profile_repair_complete": "repair_attempted",
    "profile_repair_start": "repair_attempted",
    "recovery_prompt_saved": "recovery_handoff_saved",
    "run_start": "request_understood",
    "run_stop": "diagnostic_emitted",
    "scheduler_diagnostics": "diagnostic_emitted",
    "step_capability_evidence_check": "acceptance_started",
    "step_obligation_scope": "contract_loaded",
    "step_prompt_contract": "step_prompt_built",
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
    "ultra_phase_context_updated": "phase_context_attached",
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
    "ultra_plan_generation_succeeded": "plan_generated",
    "ultra_plan_generation_tool_call_rejected": "plan_linted",
    "ultra_plan_raw_output_shape": "plan_generated",
    "ultra_final_acceptance": "acceptance_started",
    "ultra_final_acceptance_failed": "acceptance_failed",
    "ultra_partial_artifact_summary": "diagnostic_emitted",
    "ultra_plan_complete": "acceptance_passed",
    "tui_command_start": "manual_tui_trace_recorded",
    "tui_command_stop": "manual_tui_trace_recorded",
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
        if subject == "source-anvildev":
            source_prompt_events = load_source_prompt_trace_events(run_root, row)
            normalized.extend(
                normalize_events(source_prompt_events, row=row, subject=subject)
            )
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


def load_source_prompt_trace_events(
    run_root: Path,
    row: dict[str, Any],
) -> list[dict[str, Any]]:
    run_id = str(row.get("run_id", ""))
    if not run_id:
        return []
    run_dir = run_root / "runs" / run_id
    events: list[dict[str, Any]] = []
    phase_index = 0
    seen_prompt_bodies: set[str] = set()
    for log_path in source_llm_io_logs(run_dir):
        for raw in read_jsonl(log_path):
            if not str(raw.get("event", "")).endswith(".request"):
                continue
            payload = raw.get("payload", {}) or {}
            if not isinstance(payload, dict):
                continue
            for message in payload.get("messages", []) or []:
                if not isinstance(message, dict):
                    continue
                if str(message.get("role", "")) != "user":
                    continue
                content = str(message.get("content", ""))
                prompt_body = content.strip()
                if not prompt_body or prompt_body in seen_prompt_bodies:
                    continue
                seen_prompt_bodies.add(prompt_body)
                if is_source_phase_context_prompt(content):
                    phase_index += 1
                    events.append(source_phase_context_event(content, phase_index))
                if is_source_step_prompt(content):
                    events.append(source_step_prompt_event(content))
    return events


def source_llm_io_logs(run_dir: Path) -> list[Path]:
    state_root = run_dir / "workdir" / ".anvil" / "state" / "sessions"
    if not state_root.exists():
        return []
    return sorted(state_root.glob("*/logs/llm-io.jsonl"))


def is_source_phase_context_prompt(content: str) -> bool:
    return (
        "Ultra goal:" in content
        and "Current phase id:" in content
        and "Current phase goal:" in content
        and "Existing workspace snapshot:" in content
    )


def is_source_step_prompt(content: str) -> bool:
    return (
        "Overall goal:" in content
        and "Current step id:" in content
        and "Verification commands for this step:" in content
        and "Expected verification result:" in content
    )


def source_phase_context_event(content: str, phase_index: int) -> dict[str, Any]:
    snapshot = section_after(content, "Existing workspace snapshot:")
    return {
        "event": "source_phase_context_observed",
        "phase_id": first_line_after(content, "Current phase id:"),
        "phase_index": phase_index,
        "has_ultra_goal": "Ultra goal:" in content,
        "has_current_phase": "Current phase id:" in content and "Current phase goal:" in content,
        "has_workspace_snapshot": "Existing workspace snapshot:" in content,
        "has_profile_contract": "Profile contract:" in content,
        "has_previous_context": phase_index > 1 and snapshot_has_context(snapshot),
        "has_prior_conversation_context": phase_index > 1,
        "prompt_body_saved": False,
    }


def source_step_prompt_event(content: str) -> dict[str, Any]:
    return {
        "event": "source_step_prompt_observed",
        "step_id": first_line_after(content, "Current step id:"),
        "has_overall_goal": "Overall goal:" in content,
        "has_expected_paths": "Expected paths after this step:" in content
        or "Expected paths for this step:" in content,
        "has_verify_commands": "Verification commands for this step:" in content,
        "has_expected_result": "Expected verification result:" in content,
        "has_bounded_repair_policy": "fix only this step" in content.lower()
        or "repair only this step" in content.lower()
        or "bounded step-local repair" in content.lower(),
        "prompt_body_saved": False,
    }


def first_line_after(content: str, marker: str) -> str:
    if marker not in content:
        return ""
    tail = content.split(marker, 1)[1]
    for line in tail.splitlines():
        text = line.strip()
        if text:
            return redact_text(text)
    return ""


def section_after(content: str, marker: str) -> str:
    if marker not in content:
        return ""
    tail = content.split(marker, 1)[1]
    parts = tail.split("\n\n", 1)
    return parts[0]


def snapshot_has_context(snapshot: str) -> bool:
    normalized = " ".join(snapshot.strip().lower().split())
    return bool(normalized and normalized not in {"- none detected", "none detected", "- none"})


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
    if name == "browser_oracle_summary":
        if event_false(event.get("browser_success")) or event_false(event.get("ok")):
            return "acceptance_failed"
        if event_bool(event.get("browser_success")) or event_bool(event.get("ok")):
            return "acceptance_passed"
    if name in {"ultra_final_acceptance", "phase_verification_result"}:
        if event_false(event.get("ok")):
            return "acceptance_failed" if name == "ultra_final_acceptance" else "verify_failed"
        if event_bool(event.get("ok")):
            return "acceptance_passed" if name == "ultra_final_acceptance" else "verify_started"
    if name == "run_stop" and event_false(event.get("ok")):
        return "diagnostic_emitted"
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
        "phase_index",
        "step_id",
        "step_kind",
        "shared_execution_session",
        "session_message_count",
        "completed_phase_count",
        "changed_path_count",
        "recent_verify_failure_count",
        "recent_repair_changed_path_count",
        "unresolved_repair_target_count",
        "has_previous_context",
        "has_prior_conversation_context",
        "has_ultra_goal",
        "has_current_phase",
        "has_workspace_snapshot",
        "has_profile_contract",
        "has_overall_goal",
        "has_expected_paths",
        "has_verify_commands",
        "has_expected_result",
        "has_bounded_repair_policy",
        "has_prior_artifact_context",
        "prior_artifact_context_applicable",
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
    source_available = trace_report_available(source)
    mvp_available = trace_report_available(mvp)
    condition = same_condition_status(source, mvp)
    gate_results = build_gate_results(
        source,
        mvp,
        source_available=source_available,
        mvp_available=mvp_available,
        same_condition=condition,
    )
    semantic_findings = trace_contract_findings(source, mvp)
    apply_contract_findings_to_gate_results(gate_results, semantic_findings)
    gate_status_counts = Counter(str(item["status"]) for item in gate_results)
    missing_stages = sorted(source_stages - mvp_stages)
    extra_stages = sorted(mvp_stages - source_stages)
    return {
        "schema_version": TRACE_REPORT_VERSION,
        "status": "compared"
        if source_available and mvp_available and condition["status"] == "match"
        else trace_diff_status(
            source_available=source_available,
            mvp_available=mvp_available,
            same_condition=condition,
        ),
        "source_trace_id": source.get("trace_id", ""),
        "mvp_trace_id": mvp.get("trace_id", ""),
        "source_trace_available": source_available,
        "mvp_trace_available": mvp_available,
        "same_condition": condition,
        "missing_stages_in_mvp": missing_stages,
        "extra_stages_in_mvp": extra_stages,
        "missing_gate_ids_in_mvp": sorted(source_gates - mvp_gates),
        "extra_gate_ids_in_mvp": sorted(mvp_gates - source_gates),
        "gate_results": gate_results,
        "gate_status_counts": dict(sorted(gate_status_counts.items())),
        "passed_gate_ids": sorted(
            item["gate_id"] for item in gate_results if item["status"] == "pass"
        ),
        "failed_gate_ids": sorted(
            item["gate_id"] for item in gate_results if item["status"] == "fail"
        ),
        "intentionally_different_gate_ids": sorted(
            item["gate_id"]
            for item in gate_results
            if item["status"] == "intentionally_different"
        ),
        "partial_gate_ids": [],
        "semantic_findings": semantic_findings,
        "regressions": trace_regressions(
            source,
            mvp,
            missing_stages=missing_stages,
            gate_results=gate_results,
            semantic_findings=semantic_findings,
        ),
        "correct_failure_detection": trace_correct_failure_detection(
            source,
            mvp,
            extra_stages=extra_stages,
        ),
    }


def trace_contract_findings(source: dict[str, Any], mvp: dict[str, Any]) -> list[dict[str, Any]]:
    findings: list[dict[str, Any]] = []
    findings.extend(phase_context_findings("source", normalized_events_for_report(source)))
    findings.extend(phase_context_findings("mvp", normalized_events_for_report(mvp)))
    findings.extend(step_prompt_findings("source", normalized_events_for_report(source)))
    findings.extend(step_prompt_findings("mvp", normalized_events_for_report(mvp)))
    return findings


def normalized_events_for_report(report: dict[str, Any]) -> list[dict[str, Any]]:
    events = report.get("normalized_events", [])
    if isinstance(events, list) and events:
        return [event for event in events if isinstance(event, dict)]
    raw_path = str(report.get("normalized_event_sequence_path", "")).strip()
    if not raw_path:
        return []
    path = Path(raw_path)
    if path.is_file():
        return read_jsonl(path)
    return []


def phase_context_findings(side: str, events: list[dict[str, Any]]) -> list[dict[str, Any]]:
    phase_events = [event for event in events if event.get("stage") == "phase_context_attached"]
    if not phase_events:
        return [
            {
                "gate_id": "G-S05",
                "side": side,
                "kind": "missing_context",
                "reason": "phase_context_trace_missing",
            }
        ]
    findings: list[dict[str, Any]] = []
    for event in phase_events:
        if event.get("source_event") == "source_phase_context_observed":
            required = [
                ("has_ultra_goal", "missing_ultra_goal"),
                ("has_current_phase", "missing_current_phase"),
                ("has_workspace_snapshot", "missing_context"),
                ("has_profile_contract", "missing_profile_contract"),
            ]
            for field, kind in required:
                if event.get(field) is False:
                    findings.append(
                        contract_finding("G-S05", side, kind, field, event)
                    )
        phase_index = int_or_zero(event.get("phase_index"))
        has_prior_context_field = (
            "has_previous_context" in event or "has_prior_conversation_context" in event
        )
        prior_context_present = event_bool(event.get("has_previous_context")) or event_bool(
            event.get("has_prior_conversation_context")
        )
        if phase_index > 1 and has_prior_context_field and not prior_context_present:
            findings.append(
                contract_finding(
                    "G-S05",
                    side,
                    "missing_context",
                    "has_previous_context",
                    event,
                )
            )
    return findings


def step_prompt_findings(side: str, events: list[dict[str, Any]]) -> list[dict[str, Any]]:
    step_events = [event for event in events if event.get("stage") == "step_prompt_built"]
    if not step_events:
        return [
            {
                "gate_id": "G-S06",
                "side": side,
                "kind": "missing_step_prompt",
                "reason": "step_prompt_trace_missing",
            }
        ]
    required = [
        ("has_overall_goal", "missing_overall_goal"),
        ("has_expected_paths", "missing_expected_paths"),
        ("has_verify_commands", "missing_verify"),
        ("has_expected_result", "missing_expected_result"),
    ]
    findings: list[dict[str, Any]] = []
    for event in step_events:
        for field, kind in required:
            if event.get(field) is False:
                findings.append(contract_finding("G-S06", side, kind, field, event))
    return findings


def contract_finding(
    gate_id: str,
    side: str,
    kind: str,
    field: str,
    event: dict[str, Any],
) -> dict[str, Any]:
    return {
        "gate_id": gate_id,
        "side": side,
        "kind": kind,
        "field": field,
        "run_id": str(event.get("run_id", "")),
        "source_event": str(event.get("source_event", "")),
        "phase_id": str(event.get("phase_id", "")),
        "step_id": str(event.get("step_id", "")),
    }


def apply_contract_findings_to_gate_results(
    gate_results: list[dict[str, Any]],
    findings: list[dict[str, Any]],
) -> None:
    gates_with_findings = {str(item.get("gate_id", "")) for item in findings}
    for item in gate_results:
        if item["gate_id"] not in gates_with_findings:
            continue
        item["status"] = "fail"
        item["reason"] = "semantic_trace_contract_missing"
        item["semantic_finding_count"] = sum(
            1 for finding in findings if finding.get("gate_id") == item["gate_id"]
        )


def trace_report_available(report: dict[str, Any]) -> bool:
    if not isinstance(report, dict) or not report:
        return False
    count = int_or_none(report.get("normalized_event_count"))
    if count is not None and count > 0:
        return True
    return bool(report.get("stage_counts") or report.get("gate_counts"))


def same_condition_status(source: dict[str, Any], mvp: dict[str, Any]) -> dict[str, Any]:
    source_signature = condition_signature(source)
    mvp_signature = condition_signature(mvp)
    if not source_signature or not mvp_signature:
        return {
            "status": "unknown",
            "source_signature_count": len(source_signature),
            "mvp_signature_count": len(mvp_signature),
            "reason": "manifest_rows_missing",
        }
    source_set = {json.dumps(item, sort_keys=True) for item in source_signature}
    mvp_set = {json.dumps(item, sort_keys=True) for item in mvp_signature}
    missing = sorted(json.loads(item) for item in source_set - mvp_set)
    extra = sorted(json.loads(item) for item in mvp_set - source_set)
    return {
        "status": "match" if not missing and not extra else "mismatch",
        "source_signature_count": len(source_signature),
        "mvp_signature_count": len(mvp_signature),
        "missing_in_mvp": missing[:20],
        "extra_in_mvp": extra[:20],
    }


def condition_signature(report: dict[str, Any]) -> list[dict[str, str]]:
    rows = report.get("manifest_rows", []) or []
    signature: list[dict[str, str]] = []
    for row in rows:
        if not isinstance(row, dict):
            continue
        signature.append(
            {
                "suite": str(row.get("suite", "")),
                "scenario": str(row.get("scenario", "")),
                "mode": str(row.get("mode", "")),
                "provider_model_pair": str(row.get("provider_model_pair", "")),
            }
        )
    return sorted(signature, key=lambda item: tuple(item.values()))


def trace_diff_status(
    *,
    source_available: bool,
    mvp_available: bool,
    same_condition: dict[str, Any],
) -> str:
    if not source_available and not mvp_available:
        return "missing_source_and_mvp_trace"
    if not source_available:
        return "missing_source_trace"
    if not mvp_available:
        return "missing_mvp_trace"
    if same_condition.get("status") == "mismatch":
        return "same_condition_mismatch"
    if same_condition.get("status") == "unknown":
        return "same_condition_unknown"
    return "compared"


def build_gate_results(
    source: dict[str, Any],
    mvp: dict[str, Any],
    *,
    source_available: bool,
    mvp_available: bool,
    same_condition: dict[str, Any],
) -> list[dict[str, Any]]:
    source_counts = source.get("gate_counts", {}) or {}
    mvp_counts = mvp.get("gate_counts", {}) or {}
    source_stage_counts = source.get("stage_counts", {}) or {}
    mvp_stage_counts = mvp.get("stage_counts", {}) or {}
    stage_by_gate = stages_by_gate()
    results: list[dict[str, Any]] = []
    for gate_id in REQUIRED_GATE_IDS:
        source_count = int_or_zero(source_counts.get(gate_id))
        mvp_count = int_or_zero(mvp_counts.get(gate_id))
        stages = stage_by_gate.get(gate_id, [])
        source_stages = [stage for stage in stages if int_or_zero(source_stage_counts.get(stage))]
        mvp_stages = [stage for stage in stages if int_or_zero(mvp_stage_counts.get(stage))]
        status = "pass"
        reason = "source_and_mvp_gate_observed"
        if not source_available:
            status = "fail"
            reason = "source_trace_missing"
        elif not mvp_available:
            status = "fail"
            reason = "mvp_trace_missing"
        elif same_condition.get("status") == "mismatch":
            status = "fail"
            reason = "same_condition_mismatch"
        elif same_condition.get("status") == "unknown":
            status = "fail"
            reason = "same_condition_unknown"
        elif source_count > 0 and mvp_count > 0:
            status = "pass"
            reason = "source_and_mvp_gate_observed"
        elif source_count > 0:
            status = "fail"
            reason = "missing_gate_in_mvp_trace"
        elif mvp_count > 0:
            status = "fail"
            reason = "source_gate_not_observed_in_trace"
        else:
            status = "fail"
            reason = "gate_not_observed_in_trace"
        results.append(
            {
                "gate_id": gate_id,
                "status": status,
                "reason": reason,
                "source_event_count": source_count,
                "mvp_event_count": mvp_count,
                "source_stages": source_stages,
                "mvp_stages": mvp_stages,
            }
        )
    return results


def stages_by_gate() -> dict[str, list[str]]:
    out: dict[str, list[str]] = {gate_id: [] for gate_id in REQUIRED_GATE_IDS}
    for stage, gate_ids in STAGE_TO_GATE_IDS.items():
        for gate_id in gate_ids:
            out.setdefault(gate_id, []).append(stage)
    return {gate_id: sorted(set(stages)) for gate_id, stages in out.items()}


def trace_regressions(
    source: dict[str, Any],
    mvp: dict[str, Any],
    *,
    missing_stages: list[str],
    gate_results: list[dict[str, Any]],
    semantic_findings: list[dict[str, Any]],
) -> list[dict[str, Any]]:
    source_counts = source.get("stage_counts", {}) or {}
    mvp_counts = mvp.get("stage_counts", {}) or {}
    regressions: list[dict[str, Any]] = []
    for stage in missing_stages:
        regressions.append(
            {
                "kind": "missing_stage_in_mvp_trace",
                "stage": stage,
                "source": int_or_zero(source_counts.get(stage)),
                "mvp": int_or_zero(mvp_counts.get(stage)),
            }
        )
    for item in gate_results:
        if item["status"] == "fail" and item["reason"] == "missing_gate_in_mvp_trace":
            regressions.append(
                {
                    "kind": "missing_gate_in_mvp_trace",
                    "gate_id": item["gate_id"],
                    "source": item["source_event_count"],
                    "mvp": item["mvp_event_count"],
                }
            )
    for finding in semantic_findings:
        regressions.append(
            {
                "kind": finding.get("kind", "semantic_trace_contract_missing"),
                "gate_id": finding.get("gate_id", ""),
                "side": finding.get("side", ""),
                "field": finding.get("field", ""),
                "run_id": finding.get("run_id", ""),
                "source_event": finding.get("source_event", ""),
            }
        )
    return regressions


def trace_correct_failure_detection(
    source: dict[str, Any],
    mvp: dict[str, Any],
    *,
    extra_stages: list[str],
) -> list[dict[str, Any]]:
    source_counts = source.get("stage_counts", {}) or {}
    mvp_counts = mvp.get("stage_counts", {}) or {}
    detections: list[dict[str, Any]] = []
    for stage in extra_stages:
        if stage not in FAILURE_STAGES:
            continue
        detections.append(
            {
                "kind": "mvp_extra_failure_detection_stage",
                "stage": stage,
                "source": int_or_zero(source_counts.get(stage)),
                "mvp": int_or_zero(mvp_counts.get(stage)),
            }
        )
    return detections


def int_or_zero(value: Any) -> int:
    parsed = int_or_none(value)
    return parsed if parsed is not None else 0


def int_or_none(value: Any) -> int | None:
    try:
        return int(value)
    except (TypeError, ValueError):
        return None


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
