from __future__ import annotations

import csv
import json
from pathlib import Path
from typing import Any


SUMMARY_HEADER = [
    "run_id",
    "suite",
    "scenario",
    "size",
    "category",
    "mode",
    "main_provider",
    "main_model",
    "planner_provider",
    "planner_model",
    "local_llm_used",
    "rc",
    "success",
    "legacy_success",
    "process_success",
    "artifact_success",
    "build_success",
    "launch_success",
    "behavior_success",
    "source_semantic_success",
    "source_semantic_score",
    "plan_output_adherence_success",
    "plan_output_adherence_score",
    "plan_output_failure_kind",
    "plan_capability_contract_score",
    "plan_capability_oracle_version",
    "prompt_plan_capability_coverage_score",
    "prompt_plan_missing_capability_count",
    "plan_required_capability_count",
    "plan_verify_declared_coverage_score",
    "executed_verify_coverage_score",
    "plan_verify_coverage_score",
    "plan_verified_capability_count",
    "plan_unverified_capability_count",
    "prompt_plan_gap_kind",
    "plan_verify_gap_kind",
    "plan_verify_oracle_version",
    "verify_adequacy_cap_reason",
    "acceptance_confidence_score",
    "acceptance_confidence_reason",
    "prompt_contract_success",
    "acceptance_success",
    "acceptance_failure_kind",
    "acceptance_false_positive",
    "oracle_gap_kind",
    "acceptance_oracle_version",
    "queue_wait_sec",
    "process_elapsed_sec",
    "exec_elapsed_sec",
    "model_elapsed_sec",
    "tool_elapsed_sec",
    "postcheck_elapsed_sec",
    "dependency_elapsed_sec",
    "iterations",
    "tool_calls",
    "files_changed",
    "stop_reason",
    "last_blocking_reason",
    "missing_artifacts",
    "verify_attempts",
    "required_capability_count",
    "missing_capability_count",
    "required_evidence_count",
    "missing_evidence_count",
    "weak_evidence_count",
    "runtime_acceptance_primary_reason",
    "last_provider_error_kind",
    "last_provider_http_status",
    "provider_attempts",
    "fallback_decision",
    "planner_stage",
    "planner_error_kind",
    "planner_error_count",
    "planner_repair_attempts",
    "planner_schema_repaired",
    "planner_raw_schema_violation",
    "planner_parser_limitation",
    "planner_prompt_issue",
    "planner_quality_issue_count",
    "planner_retryable_quality_count",
    "planner_advisory_quality_count",
    "planner_quality_retry_count",
    "planner_quality_retry_degraded_count",
    "valid_plan_generated",
    "failure_layer",
    "capability_failure_included",
    "plan_quality_score",
    "executable_plan_score",
    "constraint_coverage_score",
    "verify_strength_score",
    "verify_adequacy_score",
    "semantic_verify_coverage_score",
    "behavior_oracle_declared_score",
    "contentless_verify_penalty",
    "artifact_ownership_score",
    "lint_repair_score",
    "execution_shape_readiness_score",
    "plan_run_predictive_score",
    "plan_run_readiness_score",
    "verify_policy_readiness_score",
    "contract_handoff_score",
    "declared_contract_completeness_score",
    "runner_handoff_integrity_score",
    "postcheck_contract_alignment_score",
    "dependency_ordering_score",
    "finalization_readiness_score",
    "readiness_blocking_issue_count",
    "readiness_warning_count",
    "readiness_cap_reason",
    "readiness_source",
    "plan_run_missed_predictive_signal",
    "missed_predictive_signal_reason",
    "readiness_false_positive_kind",
    "readiness_false_negative_kind",
    "ultra_phase_readiness_min_score",
    "ultra_phase_readiness_avg_score",
    "ultra_phase_readiness_failing_phase",
    "ultra_phase_readiness_cap_reason",
    "runtime_friction_score",
    "runtime_friction_raw_score",
    "runtime_friction_reason",
    "artifact_progress_score",
    "finalization_score",
    "finalization_reason",
    "step_finalization_score",
    "plan_finalization_score",
    "deferred_verify_finalization_score",
    "postcheck_finalization_score",
    "tool_policy_compatibility_score",
    "plan_run_runtime_health_score",
    "prompt_contract_score",
    "dependency_contract_score",
    "config_contract_score",
    "verify_contract_score",
    "postcheck_stability_score",
    "postcheck_stability_reason",
    "execution_contract_adherence_raw_score",
    "execution_contract_adherence_score",
    "execution_contract_min_subscore",
    "execution_contract_cap_reason",
    "step_obligation_scope_score",
    "step_obligation_scope_violation_count",
    "phase_completion_score",
    "phase_plan_validity_score",
    "phase_scaffold_success_score",
    "phase_step_execution_score",
    "phase_verify_success_score",
    "phase_postcheck_success_score",
    "phase_finalization_score",
    "phase_failure_stage",
    "build_verify_pass_score",
    "build_repair_effectiveness_score",
    "compile_diagnostic_progress_score",
    "verify_repair_edit_score",
    "ultra_runtime_health_score",
    "stability_score",
    "ultra_phase_quality_score",
    "execution_score",
    "time_score",
    "overall_score",
    "workdir",
    "plan_artifacts",
    "extras_json",
]


def empty_summary_row(spec: dict[str, Any]) -> dict[str, Any]:
    scenario = spec["scenario"]
    return {
        "run_id": spec["run_id"],
        "suite": spec["suite"],
        "scenario": scenario["id"],
        "size": scenario["size"],
        "category": scenario["category"],
        "mode": spec["mode"],
        "main_provider": spec["main"]["provider"],
        "main_model": spec["main"]["model"],
        "planner_provider": spec["planner"]["provider"],
        "planner_model": spec["planner"]["model"],
        "local_llm_used": str(bool(spec["local_llm_used"])).lower(),
        "rc": "",
        "success": "",
        "legacy_success": "",
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
        "prompt_plan_gap_kind": "",
        "plan_verify_gap_kind": "",
        "plan_verify_oracle_version": "",
        "verify_adequacy_cap_reason": "",
        "acceptance_confidence_score": "",
        "acceptance_confidence_reason": "",
        "prompt_contract_success": "",
        "acceptance_success": "",
        "acceptance_failure_kind": "",
        "acceptance_false_positive": "",
        "oracle_gap_kind": "",
        "acceptance_oracle_version": "",
        "queue_wait_sec": "",
        "process_elapsed_sec": "",
        "exec_elapsed_sec": "",
        "model_elapsed_sec": "",
        "tool_elapsed_sec": "",
        "postcheck_elapsed_sec": "",
        "dependency_elapsed_sec": "",
        "iterations": "",
        "tool_calls": "",
        "files_changed": "",
        "stop_reason": "",
        "last_blocking_reason": "",
        "missing_artifacts": "",
        "verify_attempts": "",
        "required_capability_count": "",
        "missing_capability_count": "",
        "required_evidence_count": "",
        "missing_evidence_count": "",
        "weak_evidence_count": "",
        "runtime_acceptance_primary_reason": "",
        "last_provider_error_kind": "",
        "last_provider_http_status": "",
        "provider_attempts": "",
        "fallback_decision": "",
        "planner_stage": "",
        "planner_error_kind": "",
        "planner_error_count": "",
        "planner_repair_attempts": "",
        "planner_schema_repaired": "",
        "planner_raw_schema_violation": "",
        "planner_parser_limitation": "",
        "planner_prompt_issue": "",
        "planner_quality_issue_count": "",
        "planner_retryable_quality_count": "",
        "planner_advisory_quality_count": "",
        "planner_quality_retry_count": "",
        "planner_quality_retry_degraded_count": "",
        "valid_plan_generated": "",
        "failure_layer": "",
        "capability_failure_included": "",
        "plan_quality_score": "",
        "executable_plan_score": "",
        "constraint_coverage_score": "",
        "verify_strength_score": "",
        "verify_adequacy_score": "",
        "semantic_verify_coverage_score": "",
        "behavior_oracle_declared_score": "",
        "contentless_verify_penalty": "",
        "artifact_ownership_score": "",
        "lint_repair_score": "",
        "execution_shape_readiness_score": "",
        "plan_run_predictive_score": "",
        "plan_run_readiness_score": "",
        "verify_policy_readiness_score": "",
        "contract_handoff_score": "",
        "declared_contract_completeness_score": "",
        "runner_handoff_integrity_score": "",
        "postcheck_contract_alignment_score": "",
        "dependency_ordering_score": "",
        "finalization_readiness_score": "",
        "readiness_blocking_issue_count": "",
        "readiness_warning_count": "",
        "readiness_cap_reason": "",
        "readiness_source": "",
        "plan_run_missed_predictive_signal": "",
        "missed_predictive_signal_reason": "",
        "readiness_false_positive_kind": "",
        "readiness_false_negative_kind": "",
        "ultra_phase_readiness_min_score": "",
        "ultra_phase_readiness_avg_score": "",
        "ultra_phase_readiness_failing_phase": "",
        "ultra_phase_readiness_cap_reason": "",
        "runtime_friction_score": "",
        "runtime_friction_raw_score": "",
        "runtime_friction_reason": "",
        "artifact_progress_score": "",
        "finalization_score": "",
        "finalization_reason": "",
        "step_finalization_score": "",
        "plan_finalization_score": "",
        "deferred_verify_finalization_score": "",
        "postcheck_finalization_score": "",
        "tool_policy_compatibility_score": "",
        "plan_run_runtime_health_score": "",
        "prompt_contract_score": "",
        "dependency_contract_score": "",
        "config_contract_score": "",
        "verify_contract_score": "",
        "postcheck_stability_score": "",
        "postcheck_stability_reason": "",
        "execution_contract_adherence_raw_score": "",
        "execution_contract_adherence_score": "",
        "execution_contract_min_subscore": "",
        "execution_contract_cap_reason": "",
        "step_obligation_scope_score": "",
        "step_obligation_scope_violation_count": "",
        "phase_completion_score": "",
        "phase_plan_validity_score": "",
        "phase_scaffold_success_score": "",
        "phase_step_execution_score": "",
        "phase_verify_success_score": "",
        "phase_postcheck_success_score": "",
        "phase_finalization_score": "",
        "phase_failure_stage": "",
        "build_verify_pass_score": "",
        "build_repair_effectiveness_score": "",
        "compile_diagnostic_progress_score": "",
        "verify_repair_edit_score": "",
        "ultra_runtime_health_score": "",
        "stability_score": "",
        "ultra_phase_quality_score": "",
        "execution_score": "",
        "time_score": "",
        "overall_score": "",
        "workdir": "",
        "plan_artifacts": "",
        "extras_json": "",
    }


def write_summary(path: Path, rows: list[dict[str, Any]]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open("w", encoding="utf-8", newline="") as f:
        writer = csv.DictWriter(f, fieldnames=SUMMARY_HEADER, delimiter="\t", extrasaction="ignore")
        writer.writeheader()
        for row in rows:
            writer.writerow({key: serialize_cell(row.get(key, "")) for key in SUMMARY_HEADER})


def read_summary(path: Path) -> list[dict[str, str]]:
    with path.open(encoding="utf-8", newline="") as f:
        reader = csv.DictReader(f, delimiter="\t")
        fieldnames = reader.fieldnames or []
        projected_header = [key for key in SUMMARY_HEADER if key in fieldnames]
        if fieldnames != projected_header:
            raise ValueError(f"unsupported summary header in {path}: {reader.fieldnames}")
        rows = list(reader)
        for row in rows:
            for key in SUMMARY_HEADER:
                row.setdefault(key, "")
        return rows


def serialize_cell(value: Any) -> str:
    if value is None:
        return ""
    if isinstance(value, bool):
        return str(value).lower()
    if isinstance(value, (dict, list)):
        return json.dumps(value, ensure_ascii=False, sort_keys=True)
    return str(value)


def calculate_overall(
    mode: str,
    plan_score: float | None,
    ultra_score: float | None,
    execution_score: float,
    time_score: float,
    executable_score: float | None = None,
    constraint_score: float | None = None,
    verify_strength_score: float | None = None,
    artifact_ownership_score: float | None = None,
    lint_repair_score: float | None = None,
    ultra_runtime_health_score: float | None = None,
) -> float:
    if mode == "minimal-loop":
        return round(0.80 * execution_score + 0.20 * time_score, 1)
    if mode == "step-plan":
        return round(
            0.35 * (plan_score or 0)
            + 0.20 * (executable_score or 0)
            + 0.15 * (constraint_score or 0)
            + 0.10 * (verify_strength_score or 0)
            + 0.10 * (artifact_ownership_score or 0)
            + 0.10 * (lint_repair_score or 0),
            1,
        )
    if mode == "plan-run":
        return round(
            0.18 * (plan_score or 0)
            + 0.12 * (executable_score or 0)
            + 0.08 * (constraint_score or 0)
            + 0.05 * (verify_strength_score or 0)
            + 0.05 * (artifact_ownership_score or 0)
            + 0.05 * (lint_repair_score or 0)
            + 0.37 * execution_score
            + 0.10 * time_score,
            1,
        )
    if mode == "ultra-plan-run":
        if ultra_runtime_health_score is None:
            return round(
                0.35 * (ultra_score or 0) + 0.55 * execution_score + 0.10 * time_score,
                1,
            )
        return round(
            0.20 * (ultra_score or 0)
            + 0.25 * ultra_runtime_health_score
            + 0.45 * execution_score
            + 0.10 * time_score,
            1,
        )
    if mode == "ultra-step-run":
        return round(
            0.25 * (plan_score or 0)
            + 0.10 * (executable_score or 0)
            + 0.08 * (constraint_score or 0)
            + 0.05 * (verify_strength_score or 0)
            + 0.05 * (artifact_ownership_score or 0)
            + 0.05 * (lint_repair_score or 0)
            + 0.32 * execution_score
            + 0.10 * time_score,
            1,
        )
    return execution_score


def calculate_plan_run_predictive_score(
    executable_score: float | None,
    artifact_ownership_score: float | None,
    verify_strength_score: float | None,
    constraint_score: float | None,
    lint_repair_score: float | None,
    execution_shape_readiness_score: float | None,
) -> float:
    return round(
        0.30 * (execution_shape_readiness_score or 0)
        + 0.25 * (executable_score or 0)
        + 0.15 * (artifact_ownership_score or 0)
        + 0.15 * (verify_strength_score or 0)
        + 0.10 * (constraint_score or 0)
        + 0.05 * (lint_repair_score or 0),
        1,
    )
