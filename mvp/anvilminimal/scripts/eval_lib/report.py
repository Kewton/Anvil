from __future__ import annotations

import statistics
import json
from collections import Counter, defaultdict
from pathlib import Path
from typing import Iterable

from .failure_classification import capability_failure_included, failure_layer_for_kind
from .run_summary import read_summary


def generate_report(run_root: Path) -> str:
    rows = read_summary(run_root / "summary.eval.tsv")
    lines = ["# anvilminimal Eval Report", ""]
    lines.extend(acceptance_summary(rows))
    lines.extend(schema_summary(rows))
    lines.extend(speed_diagnostics_summary(rows))
    lines.extend(section_table("Mode Summary", aggregate(rows, "mode")))
    lines.extend(section_table("Size Summary", aggregate(rows, "size")))
    lines.extend(section_table("Model Profile Summary", aggregate(rows, "main_provider")))
    lines.extend(core_metric_summary(rows))
    lines.extend(target_metric_summary(rows))
    lines.extend(plan_rankings(rows))
    lines.extend(executable_plan_rankings(rows))
    lines.extend(additional_plan_metric_rankings(rows))
    lines.extend(stability_summary(rows))
    lines.extend(plan_run_predictiveness_summary(rows))
    lines.extend(plan_run_readiness_summary(rows))
    lines.extend(blocking_summary(rows))
    lines.extend(stop_reason_summary(rows))
    lines.extend(planner_failure_summary(rows))
    lines.extend(planner_repair_summary(rows))
    lines.extend(planner_raw_shape_summary(rows))
    lines.extend(planner_quality_warning_summary(rows))
    lines.extend(planner_quality_issue_summary(rows))
    lines.extend(metric_reason_summary(rows))
    lines.extend(failure_layer_summary(rows))
    lines.extend(failure_summary(rows))
    return "\n".join(lines) + "\n"


def aggregate(rows: list[dict[str, str]], key: str) -> list[dict[str, str]]:
    groups: dict[str, list[dict[str, str]]] = defaultdict(list)
    for row in rows:
        groups[row.get(key, "")].append(row)
    out = []
    for name, group in sorted(groups.items()):
        elapsed = [to_float(row.get("exec_elapsed_sec")) for row in group if to_float(row.get("exec_elapsed_sec")) is not None]
        success_count = sum(1 for row in group if row.get("success") == "true")
        out.append(
            {
                "group": name,
                "success": f"{success_count}/{len(group)}",
                "acceptance": acceptance_count_cell(group),
                "false_positive": str(sum(1 for row in group if row.get("acceptance_false_positive") == "true")),
                "p50_exec_sec": fmt(percentile(elapsed, 50)),
                "p90_exec_sec": fmt(percentile(elapsed, 90)),
                "avg_score": fmt(mean([to_float(row.get("overall_score")) for row in group])),
            }
        )
    return out


def core_metric_summary(rows: list[dict[str, str]]) -> list[str]:
    groups: dict[str, list[dict[str, str]]] = defaultdict(list)
    for row in rows:
        groups[row.get("mode", "")].append(row)
    out = []
    for mode, group in sorted(groups.items()):
        elapsed = [
            to_float(row.get("exec_elapsed_sec"))
            for row in group
            if to_float(row.get("exec_elapsed_sec")) is not None
        ]
        out.append(
            {
                "mode": mode,
                "success": success_count_cell(group),
                "acceptance": acceptance_count_cell(group),
                "false_positive": str(sum(1 for row in group if row.get("acceptance_false_positive") == "true")),
                "valid_plan": valid_plan_count_cell(group),
                "plan_quality": fmt(mean([to_float(row.get("plan_quality_score")) for row in group])),
                "prompt_plan": fmt(
                    mean([to_float(row.get("prompt_plan_capability_coverage_score")) for row in group])
                ),
                "plan_verify": fmt(mean([to_float(row.get("plan_verify_coverage_score")) for row in group])),
                "confidence": fmt(mean([to_float(row.get("acceptance_confidence_score")) for row in group])),
                "shape_readiness": fmt(
                    mean([to_float(row.get("execution_shape_readiness_score")) for row in group])
                ),
                "predictive": fmt(mean([to_float(row.get("plan_run_predictive_score")) for row in group])),
                "readiness": fmt(mean([to_float(row.get("plan_run_readiness_score")) for row in group])),
                "runtime_health": fmt(
                    mean([to_float(row.get("plan_run_runtime_health_score")) for row in group])
                ),
                "postcheck": fmt(mean([to_float(row.get("postcheck_stability_score")) for row in group])),
                "finalization": fmt(mean([to_float(row.get("finalization_score")) for row in group])),
                "eca": fmt(mean([to_float(row.get("execution_contract_adherence_score")) for row in group])),
                "ultra_runtime": fmt(
                    mean([to_float(row.get("ultra_runtime_health_score")) for row in group])
                ),
                "phase_completion": fmt(
                    mean([to_float(row.get("phase_completion_score")) for row in group])
                ),
                "prompt_contract": fmt(
                    mean([to_float(row.get("prompt_contract_score")) for row in group])
                ),
                "obligation_scope": fmt(
                    mean([to_float(row.get("step_obligation_scope_score")) for row in group])
                ),
                "p50_exec_sec": fmt(percentile(elapsed, 50)),
            }
        )
    lines = ["## Core Metrics", ""]
    if not out:
        return lines + ["No rows.", ""]
    return lines + table_rows(
        out,
        [
            "mode",
            "success",
            "acceptance",
            "false_positive",
            "valid_plan",
            "plan_quality",
            "prompt_plan",
            "plan_verify",
            "confidence",
            "shape_readiness",
            "predictive",
            "readiness",
            "runtime_health",
            "postcheck",
            "finalization",
            "eca",
            "ultra_runtime",
            "phase_completion",
            "prompt_contract",
            "obligation_scope",
            "p50_exec_sec",
        ],
    )


def target_metric_summary(rows: list[dict[str, str]]) -> list[str]:
    metrics = [
        "verify_adequacy_score",
        "semantic_verify_coverage_score",
        "behavior_oracle_declared_score",
        "contentless_verify_penalty",
        "postcheck_stability_score",
        "phase_completion_score",
        "runtime_friction_score",
        "finalization_score",
        "execution_contract_adherence_raw_score",
        "execution_contract_adherence_score",
    ]
    out = []
    for mode, group in sorted(group_rows(rows, "mode").items()):
        success_rows = [row for row in group if row.get("success") == "true"]
        failure_rows = [
            row
            for row in group
            if row.get("success") != "true"
            and row.get("capability_failure_included", capability_cell(row)) != "false"
        ]
        for metric in metrics:
            out.append(
                {
                    "mode": mode,
                    "metric": metric,
                    "success_avg": fmt(mean([to_float(row.get(metric)) for row in success_rows])),
                    "failure_avg": fmt(mean([to_float(row.get(metric)) for row in failure_rows])),
                    "failure_n": str(len(failure_rows)),
                }
            )
    lines = ["## Target Runtime Metrics", ""]
    if not out:
        return lines + ["No target runtime metrics.", ""]
    return lines + table_rows(out, ["mode", "metric", "success_avg", "failure_avg", "failure_n"])


def acceptance_summary(rows: list[dict[str, str]]) -> list[str]:
    groups = group_rows(rows, "mode")
    out = []
    failure_counter = Counter()
    gap_counter = Counter()
    for mode, group in sorted(groups.items()):
        scoped = [row for row in group if row.get("acceptance_success") not in {"", None}]
        if not scoped:
            continue
        false_positive = [row for row in scoped if row.get("acceptance_false_positive") == "true"]
        out.append(
            {
                "mode": mode,
                "legacy_success": success_count_cell(group),
                "acceptance_success": acceptance_count_cell(group),
                "capability_acceptance": capability_acceptance_count_cell(group),
                "false_positive": str(len(false_positive)),
                "source_semantic_avg": fmt(mean([to_float(row.get("source_semantic_score")) for row in scoped])),
                "plan_output_avg": fmt(mean([to_float(row.get("plan_output_adherence_score")) for row in scoped])),
                "prompt_plan_avg": fmt(mean([to_float(row.get("prompt_plan_capability_coverage_score")) for row in scoped])),
                "plan_verify_avg": fmt(mean([to_float(row.get("plan_verify_coverage_score")) for row in scoped])),
                "confidence_avg": fmt(mean([to_float(row.get("acceptance_confidence_score")) for row in scoped])),
            }
        )
        for row in scoped:
            if row.get("acceptance_failure_kind"):
                failure_counter[row["acceptance_failure_kind"]] += 1
            for reason in split_reasons(row.get("acceptance_failure_reasons", "")):
                failure_counter[f"reason:{reason}"] += 1
            if row.get("oracle_gap_kind"):
                gap_counter[row["oracle_gap_kind"]] += 1
    lines = ["## Acceptance Outcomes", ""]
    if not out:
        return lines + ["No acceptance outcome rows.", ""]
    lines.extend(table_rows(out, ["mode", "legacy_success", "acceptance_success", "capability_acceptance", "false_positive", "source_semantic_avg", "plan_output_avg", "prompt_plan_avg", "plan_verify_avg", "confidence_avg"]))
    if failure_counter:
        lines.extend(table_rows(
            [{"kind": kind, "count": str(count)} for kind, count in sorted(failure_counter.items())],
            ["kind", "count"],
        ))
    if gap_counter:
        lines.extend(table_rows(
            [{"gap": gap, "count": str(count)} for gap, count in sorted(gap_counter.items())],
            ["gap", "count"],
        ))
    lines.extend(capability_contract_outcomes(rows))
    return lines


def schema_summary(rows: list[dict[str, str]]) -> list[str]:
    counter = Counter(row.get("eval_schema_version", "legacy") or "legacy" for row in rows)
    lines = ["## Eval Schema", ""]
    return lines + table_rows(
        [{"schema": schema, "count": str(count)} for schema, count in sorted(counter.items())],
        ["schema", "count"],
    )


def speed_diagnostics_summary(rows: list[dict[str, str]]) -> list[str]:
    out = []
    for mode, group in sorted(group_rows(rows, "mode").items()):
        out.append(
            {
                "mode": mode,
                "p50_wall_sec": fmt(percentile([to_float(row.get("wall_clock_sec")) for row in group], 50)),
                "p90_wall_sec": fmt(percentile([to_float(row.get("wall_clock_sec")) for row in group], 90)),
                "p50_queue_sec": fmt(percentile([to_float(row.get("queue_wait_sec")) for row in group], 50)),
                "p90_provider_wait_sec": fmt(percentile([to_float(row.get("provider_wait_sec")) for row in group], 90)),
                "p50_acceptance_oracle_sec": fmt(percentile([to_float(row.get("acceptance_oracle_sec")) for row in group], 50)),
                "provider_limit": most_common(group, "provider_limit"),
                "parallel_limit": most_common(group, "parallel_limit"),
                "lane": most_common(group, "scheduler_lane"),
            }
        )
    lines = ["## Speed Diagnostics", ""]
    if not out:
        return lines + ["No speed diagnostics.", ""]
    return lines + table_rows(
        out,
        [
            "mode",
            "p50_wall_sec",
            "p90_wall_sec",
            "p50_queue_sec",
            "p90_provider_wait_sec",
            "p50_acceptance_oracle_sec",
            "provider_limit",
            "parallel_limit",
            "lane",
        ],
    )


def capability_contract_outcomes(rows: list[dict[str, str]]) -> list[str]:
    out = []
    for mode, group in sorted(group_rows(rows, "mode").items()):
        scoped = [
            row
            for row in group
            if any(
                row.get(field)
                for field in [
                    "prompt_plan_capability_coverage_score",
                    "plan_verify_coverage_score",
                    "plan_output_adherence_score",
                ]
            )
        ]
        if not scoped:
            continue
        out.append(
            {
                "mode": mode,
                "prompt_plan_avg": fmt(mean([to_float(row.get("prompt_plan_capability_coverage_score")) for row in scoped])),
                "plan_contract_avg": fmt(mean([to_float(row.get("plan_capability_contract_score")) for row in scoped])),
                "plan_verify_avg": fmt(mean([to_float(row.get("plan_verify_coverage_score")) for row in scoped])),
                "declared_verify_avg": fmt(mean([to_float(row.get("plan_verify_declared_coverage_score")) for row in scoped])),
                "executed_verify_avg": fmt(mean([to_float(row.get("executed_verify_coverage_score")) for row in scoped])),
                "plan_output_avg": fmt(mean([to_float(row.get("plan_output_adherence_score")) for row in scoped])),
                "missing_plan": str(sum(safe_int(row.get("prompt_plan_missing_capability_count")) for row in scoped)),
                "missing_verify": str(sum(safe_int(row.get("plan_unverified_capability_count")) for row in scoped)),
            }
        )
    lines = ["", "## Capability Contract Outcomes", ""]
    if not out:
        return lines + ["No capability contract rows.", ""]
    lines.extend(table_rows(
        out,
        [
            "mode",
            "prompt_plan_avg",
            "plan_contract_avg",
            "plan_verify_avg",
            "declared_verify_avg",
            "executed_verify_avg",
            "plan_output_avg",
            "missing_plan",
            "missing_verify",
        ],
    ))
    counters = []
    for label, field in [
        ("prompt_plan_gap", "prompt_plan_gap_kind"),
        ("plan_verify_gap", "plan_verify_gap_kind"),
        ("confidence_reason", "acceptance_confidence_reason"),
    ]:
        counter = Counter()
        for row in rows:
            for reason in split_reasons(row.get(field, "")):
                counter[reason] += 1
        for reason, count in sorted(counter.items()):
            counters.append({"kind": label, "reason": reason, "count": str(count)})
    if counters:
        lines.extend(table_rows(counters, ["kind", "reason", "count"]))
    return lines


def section_table(title: str, rows: list[dict[str, str]]) -> list[str]:
    lines = [f"## {title}", ""]
    if not rows:
        return lines + ["No rows.", ""]
    headers = list(rows[0].keys())
    lines.append("| " + " | ".join(headers) + " |")
    lines.append("|" + "|".join("---" for _ in headers) + "|")
    for row in rows:
        lines.append("| " + " | ".join(str(row.get(h, "")) for h in headers) + " |")
    lines.append("")
    return lines


def plan_rankings(rows: list[dict[str, str]]) -> list[str]:
    scored = [(to_float(row.get("plan_quality_score")), row) for row in rows]
    scored = [(score, row) for score, row in scored if score is not None]
    lines = ["## Plan Quality", ""]
    if not scored:
        return lines + ["No plan scores.", ""]
    top = sorted(scored, key=lambda item: item[0], reverse=True)[:5]
    bottom = sorted(scored, key=lambda item: item[0])[:5]
    lines.append("| rank | scenario | mode | score |")
    lines.append("|---|---|---|---|")
    for label, items in [("top", top), ("bottom", bottom)]:
        for score, row in items:
            lines.append(f"| {label} | {row['scenario']} | {row['mode']} | {fmt(score)} |")
    lines.append("")
    return lines


def executable_plan_rankings(rows: list[dict[str, str]]) -> list[str]:
    scored = [(to_float(row.get("executable_plan_score")), row) for row in rows]
    scored = [(score, row) for score, row in scored if score is not None]
    lines = ["## Executable Plan Quality", ""]
    if not scored:
        return lines + ["No executable plan scores.", ""]
    bottom = sorted(scored, key=lambda item: item[0])[:8]
    lines.append("| rank | scenario | mode | executable_score | plan_score | success |")
    lines.append("|---|---|---|---:|---:|---|")
    for score, row in bottom:
        lines.append(
            f"| bottom | {row['scenario']} | {row['mode']} | {fmt(score)} | "
            f"{row.get('plan_quality_score', '')} | {row.get('success', '')} |"
        )
    lines.append("")
    return lines


def additional_plan_metric_rankings(rows: list[dict[str, str]]) -> list[str]:
    metrics = [
        "constraint_coverage_score",
        "verify_strength_score",
        "plan_capability_contract_score",
        "prompt_plan_capability_coverage_score",
        "plan_verify_declared_coverage_score",
        "executed_verify_coverage_score",
        "plan_verify_coverage_score",
        "acceptance_confidence_score",
        "verify_adequacy_score",
        "semantic_verify_coverage_score",
        "behavior_oracle_declared_score",
        "contentless_verify_penalty",
        "artifact_ownership_score",
        "lint_repair_score",
        "plan_run_readiness_score",
        "verify_policy_readiness_score",
        "contract_handoff_score",
        "declared_contract_completeness_score",
        "runner_handoff_integrity_score",
        "postcheck_contract_alignment_score",
        "dependency_ordering_score",
        "finalization_readiness_score",
        "ultra_phase_readiness_min_score",
        "ultra_phase_readiness_avg_score",
        "runtime_friction_score",
        "runtime_friction_raw_score",
        "artifact_progress_score",
        "finalization_score",
        "step_finalization_score",
        "plan_finalization_score",
        "deferred_verify_finalization_score",
        "postcheck_finalization_score",
        "tool_policy_compatibility_score",
        "dependency_contract_score",
        "config_contract_score",
        "verify_contract_score",
        "postcheck_stability_score",
        "execution_contract_adherence_raw_score",
        "execution_contract_adherence_score",
        "execution_contract_min_subscore",
        "build_verifier_completion_score",
        "dependency_setup_boundary_score",
        "dependency_setup_bridge_score",
        "build_verifier_lifecycle_score",
        "profile_repair_symmetry_score",
        "step_runtime_bridge_score",
        "repair_target_followthrough_score",
        "plan_run_success_predictor",
        "repair_target_resolution_score",
        "repair_stagnation_score",
        "profile_static_vs_build_gap_score",
        "step_obligation_scope_score",
        "phase_completion_score",
        "phase_plan_validity_score",
        "phase_scaffold_success_score",
        "phase_step_execution_score",
        "phase_verify_success_score",
        "phase_postcheck_success_score",
        "phase_finalization_score",
        "build_verify_pass_score",
        "build_repair_effectiveness_score",
        "compile_diagnostic_progress_score",
        "verify_repair_edit_score",
        "ultra_runtime_health_score",
    ]
    lines = ["## Detailed Metric Diagnostics", ""]
    ranked_rows = []
    for metric in metrics:
        scored = [(to_float(row.get(metric)), row) for row in rows]
        scored = [(score, row) for score, row in scored if score is not None]
        for score, row in sorted(scored, key=lambda item: item[0])[:3]:
            ranked_rows.append(
                {
                    "metric": metric,
                    "scenario": row.get("scenario", ""),
                    "mode": row.get("mode", ""),
                    "provider": row.get("planner_provider", ""),
                    "score": fmt(score),
                    "success": row.get("success", ""),
                }
            )
    if not ranked_rows:
        return lines + ["No detailed metric scores.", ""]
    return lines + table_rows(ranked_rows, ["metric", "scenario", "mode", "provider", "score", "success"])


def stability_summary(rows: list[dict[str, str]]) -> list[str]:
    scored = []
    seen = set()
    for row in rows:
        score = to_float(row.get("stability_score"))
        if score is None:
            continue
        key = prediction_key(row)
        if key in seen:
            continue
        seen.add(key)
        scored.append((score, row))
    lines = ["## Stability", ""]
    if not scored:
        return lines + ["No stability scores. Run with --runs 2 or more to populate this metric.", ""]
    out = []
    for score, row in sorted(scored, key=lambda item: item[0])[:8]:
        out.append(
            {
                "scenario": row.get("scenario", ""),
                "mode": row.get("mode", ""),
                "main": f"{row.get('main_provider', '')}:{row.get('main_model', '')}",
                "planner": f"{row.get('planner_provider', '')}:{row.get('planner_model', '')}",
                "stability_score": fmt(score),
            }
        )
    return lines + table_rows(out, ["scenario", "mode", "main", "planner", "stability_score"])


def plan_run_predictiveness_summary(rows: list[dict[str, str]]) -> list[str]:
    step_groups = group_by_prediction_key([row for row in rows if row.get("mode") == "step-plan"])
    run_groups = group_by_prediction_key([row for row in rows if row.get("mode") == "plan-run"])
    pair_rows = []
    step_scores = []
    run_success_rates = []
    false_positive = 0
    false_negative = 0
    for key in sorted(set(step_groups).intersection(run_groups)):
        step_score = mean([to_float(row.get("plan_run_predictive_score")) for row in step_groups[key]])
        if step_score is None:
            step_score = mean([to_float(row.get("overall_score")) for row in step_groups[key]])
        run_success = success_rate(run_groups[key])
        if step_score is None:
            continue
        step_scores.append(step_score)
        run_success_rates.append(run_success)
        if step_score >= 80 and run_success < 100:
            false_positive += 1
        if step_score < 70 and run_success >= 80:
            false_negative += 1
        sample = step_groups[key][0]
        pair_rows.append(
            {
                "scenario": sample.get("scenario", ""),
                "main": f"{sample.get('main_provider', '')}:{sample.get('main_model', '')}",
                "planner": f"{sample.get('planner_provider', '')}:{sample.get('planner_model', '')}",
                "step_predictive": fmt(step_score),
                "plan_run_success": fmt(run_success),
            }
        )
    lines = ["## Plan Run Predictiveness", ""]
    if not pair_rows:
        return lines + ["No paired step-plan and plan-run rows.", ""]
    corr = pearson(step_scores, run_success_rates)
    lines.extend(
        [
            f"paired_rows: {len(pair_rows)}",
            f"correlation: {fmt(corr)}",
            f"false_positive: {false_positive}",
            f"false_negative: {false_negative}",
            "",
        ]
    )
    return lines + table_rows(pair_rows, ["scenario", "main", "planner", "step_predictive", "plan_run_success"])


def plan_run_readiness_summary(rows: list[dict[str, str]]) -> list[str]:
    step_groups = group_by_prediction_key([row for row in rows if row.get("mode") == "step-plan"])
    run_groups = group_by_prediction_key([row for row in rows if row.get("mode") == "plan-run"])
    pair_rows = []
    readiness_scores = []
    run_success_rates = []
    false_positive = 0
    false_negative = 0
    missed_signals = Counter()
    for key in sorted(set(step_groups).intersection(run_groups)):
        readiness = mean([to_float(row.get("plan_run_readiness_score")) for row in step_groups[key]])
        run_success = success_rate(run_groups[key])
        if readiness is None:
            continue
        readiness_scores.append(readiness)
        run_success_rates.append(run_success)
        if readiness >= 80 and run_success < 100:
            false_positive += 1
        if readiness < 70 and run_success >= 80:
            false_negative += 1
        sample = step_groups[key][0]
        cap_reasons = sorted({row.get("readiness_cap_reason", "") for row in step_groups[key] if row.get("readiness_cap_reason")})
        pair_rows.append(
            {
                "scenario": sample.get("scenario", ""),
                "main": f"{sample.get('main_provider', '')}:{sample.get('main_model', '')}",
                "planner": f"{sample.get('planner_provider', '')}:{sample.get('planner_model', '')}",
                "readiness": fmt(readiness),
                "plan_run_success": fmt(run_success),
                "cap": ";".join(cap_reasons),
            }
        )
    for row in rows:
        reason = row.get("missed_predictive_signal_reason", "")
        if reason:
            missed_signals[reason] += 1
    lines = ["## Plan Run Readiness", ""]
    scored = [row for row in rows if to_float(row.get("plan_run_readiness_score")) is not None]
    if scored:
        lines.extend(
            table_rows(
                [
                    {
                        "metric": metric,
                        "avg": fmt(mean([to_float(row.get(metric)) for row in scored])),
                    }
                    for metric in [
                        "plan_run_readiness_score",
                        "verify_policy_readiness_score",
                        "contract_handoff_score",
                        "declared_contract_completeness_score",
                        "runner_handoff_integrity_score",
                        "postcheck_contract_alignment_score",
                        "dependency_ordering_score",
                        "finalization_readiness_score",
                    ]
                ],
                ["metric", "avg"],
            )
        )
    if pair_rows:
        corr = pearson(readiness_scores, run_success_rates)
        lines.extend(
            [
                f"paired_rows: {len(pair_rows)}",
                f"correlation: {fmt(corr)}",
                f"false_positive: {false_positive}",
                f"false_negative: {false_negative}",
                "",
            ]
        )
        lines.extend(table_rows(pair_rows, ["scenario", "main", "planner", "readiness", "plan_run_success", "cap"]))
    else:
        lines.extend(["No paired step-plan and plan-run readiness rows.", ""])
    if missed_signals:
        lines.extend(table_rows(
            [{"reason": reason, "count": str(count)} for reason, count in sorted(missed_signals.items())],
            ["reason", "count"],
        ))
    return lines


def failure_summary(rows: list[dict[str, str]]) -> list[str]:
    counter = Counter()
    for row in rows:
        if row.get("success") == "true":
            continue
        extras = parse_extras(row)
        if extras.get("failure_kind"):
            counter[str(extras["failure_kind"])] += 1
        elif row.get("success") == "diagnostic_skipped":
            counter["diagnostic_skipped"] += 1
        elif row.get("rc") == "124":
            counter["timeout"] += 1
        elif row.get("rc") not in {"", "0"}:
            counter["process_failure"] += 1
        else:
            counter["postcheck_failure"] += 1
    lines = ["## Failures", "", "| kind | count |", "|---|---:|"]
    if not counter:
        lines.append("| none | 0 |")
    else:
        for key, count in sorted(counter.items()):
            lines.append(f"| {key} | {count} |")
    lines.append("")
    return lines


def metric_reason_summary(rows: list[dict[str, str]]) -> list[str]:
    reason_fields = [
        ("postcheck", "postcheck_stability_reason"),
        ("eca_cap", "execution_contract_cap_reason"),
        ("runtime_friction", "runtime_friction_reason"),
        ("finalization", "finalization_reason"),
        ("phase_failure", "phase_failure_stage"),
        ("readiness_cap", "readiness_cap_reason"),
        ("missed_signal", "missed_predictive_signal_reason"),
        ("prompt_plan_gap", "prompt_plan_gap_kind"),
        ("plan_verify_gap", "plan_verify_gap_kind"),
        ("confidence", "acceptance_confidence_reason"),
    ]
    out = []
    for label, field in reason_fields:
        counter = Counter()
        for row in rows:
            for reason in split_reasons(row.get(field, "")):
                counter[reason] += 1
        if counter:
            for reason, count in sorted(counter.items()):
                out.append({"metric": label, "reason": reason, "count": str(count)})
    lines = ["## Target Metric Reasons", ""]
    if not out:
        return lines + ["No target metric reasons.", ""]
    return lines + table_rows(out, ["metric", "reason", "count"])


def failure_layer_summary(rows: list[dict[str, str]]) -> list[str]:
    counter = Counter()
    included = Counter()
    for row in rows:
        if row.get("success") == "true":
            continue
        layer = row.get("failure_layer", "")
        if not layer:
            extras = parse_extras(row)
            layer = failure_layer_for_kind(str(extras.get("failure_kind", "")))
        if not layer:
            layer = "unknown"
        counter[layer] += 1
        include_value = row.get("capability_failure_included", "")
        if include_value == "":
            extras = parse_extras(row)
            include_value = str(capability_failure_included(str(extras.get("failure_kind", "")))).lower()
        included[(layer, include_value)] += 1
    lines = ["## Failure Layers", ""]
    if not counter:
        return lines + ["No failed rows.", ""]
    out = []
    for layer, count in sorted(counter.items()):
        out.append(
            {
                "layer": layer,
                "count": str(count),
                "capability_included": str(included.get((layer, "true"), 0)),
                "capability_excluded": str(included.get((layer, "false"), 0)),
            }
        )
    return lines + table_rows(out, ["layer", "count", "capability_included", "capability_excluded"])


def planner_failure_summary(rows: list[dict[str, str]]) -> list[str]:
    counter = Counter()
    for row in rows:
        extras = parse_extras(row)
        kind = row.get("planner_error_kind") or str(extras.get("planner_error_kind", ""))
        if not kind:
            continue
        key = (
            row.get("planner_provider", ""),
            row.get("planner_model", ""),
            row.get("planner_stage") or str(extras.get("planner_stage", "")),
            kind,
        )
        counter[key] += 1
    lines = ["## Planner Failures", "", "| provider | model | stage | kind | count |", "|---|---|---|---|---:|"]
    if not counter:
        lines.append("| none |  |  |  | 0 |")
    else:
        for (provider, model, stage, kind), count in sorted(counter.items()):
            lines.append(f"| {provider} | {model} | {stage} | {kind} | {count} |")
    lines.append("")
    return lines


def planner_repair_summary(rows: list[dict[str, str]]) -> list[str]:
    repaired = [row for row in rows if row.get("planner_schema_repaired") == "true"]
    raw_violations = [row for row in rows if row.get("planner_raw_schema_violation") == "true"]
    parser_limitations = [row for row in rows if row.get("planner_parser_limitation") == "true"]
    prompt_issues = [row for row in rows if row.get("planner_prompt_issue") == "true"]
    rows_out = [
        {"metric": "schema_repaired", "count": str(len(repaired))},
        {"metric": "raw_schema_violation", "count": str(len(raw_violations))},
        {"metric": "parser_limitation", "count": str(len(parser_limitations))},
        {"metric": "prompt_issue", "count": str(len(prompt_issues))},
    ]
    lines = ["## Planner Repairs", ""]
    return lines + table_rows(rows_out, ["metric", "count"])


def planner_raw_shape_summary(rows: list[dict[str, str]]) -> list[str]:
    counter = Counter()
    for row in rows:
        extras = parse_extras(row)
        shapes = extras.get("planner_raw_output_shapes", [])
        if not isinstance(shapes, list):
            continue
        for shape in shapes:
            if not isinstance(shape, dict):
                continue
            key = (
                str(shape.get("planner_provider", "")),
                str(shape.get("planner_model", "")),
                str(shape.get("json_extract_status", "")),
                str(shape.get("has_json_object", "")),
                str(shape.get("contains_goal_key", "")),
                str(shape.get("contains_steps_key", "")),
            )
            counter[key] += 1
    lines = [
        "## Planner Raw Output Shapes",
        "",
        "| provider | model | json_extract_status | has_json_object | contains_goal_key | contains_steps_key | count |",
        "|---|---|---|---|---|---|---:|",
    ]
    if not counter:
        lines.append("| none |  |  |  |  |  | 0 |")
    else:
        for (provider, model, status, has_json, has_goal, has_steps), count in sorted(counter.items()):
            lines.append(f"| {provider} | {model} | {status} | {has_json} | {has_goal} | {has_steps} | {count} |")
    lines.append("")
    return lines


def planner_quality_warning_summary(rows: list[dict[str, str]]) -> list[str]:
    counter = Counter()
    for row in rows:
        extras = parse_extras(row)
        warnings = extras.get("planner_quality_warnings", [])
        if not isinstance(warnings, list):
            continue
        for warning in warnings:
            if not isinstance(warning, dict):
                continue
            key = (
                str(warning.get("planner_provider", "")),
                str(warning.get("planner_model", "")),
                str(warning.get("message", "")),
            )
            counter[key] += 1
    lines = [
        "## Planner Quality Warnings",
        "",
        "| provider | model | warning | count |",
        "|---|---|---|---:|",
    ]
    if not counter:
        lines.append("| none |  |  | 0 |")
    else:
        for (provider, model, message), count in sorted(counter.items()):
            lines.append(f"| {provider} | {model} | {message} | {count} |")
    lines.append("")
    return lines


def planner_quality_issue_summary(rows: list[dict[str, str]]) -> list[str]:
    counter = Counter()
    retry_count = 0
    degraded_count = 0
    for row in rows:
        extras = parse_extras(row)
        retry_count += safe_int(
            row.get("planner_quality_retry_count")
            or extras.get("planner_quality_retry_count")
        )
        degraded_count += safe_int(
            row.get("planner_quality_retry_degraded_count")
            or extras.get("planner_quality_retry_degraded_count")
        )
        issues = extras.get("planner_quality_issues", [])
        if not isinstance(issues, list):
            continue
        for issue in issues:
            if not isinstance(issue, dict):
                continue
            key = (
                str(issue.get("severity", "")),
                str(issue.get("category", "")),
                str(issue.get("message", "")),
            )
            counter[key] += 1
    lines = [
        "## Planner Quality Issues",
        "",
        f"quality_retry_count: {retry_count}",
        f"quality_retry_degraded_count: {degraded_count}",
        "",
        "| severity | category | message | count |",
        "|---|---|---|---:|",
    ]
    if not counter:
        lines.append("| none |  |  | 0 |")
    else:
        for (severity, category, message), count in sorted(counter.items()):
            lines.append(f"| {severity} | {category} | {message} | {count} |")
    lines.append("")
    return lines


def stop_reason_summary(rows: list[dict[str, str]]) -> list[str]:
    detail_rows = []
    for row in rows:
        if row.get("success") == "true":
            continue
        extras = parse_extras(row)
        detail_rows.append(
            {
                "scenario": row.get("scenario", ""),
                "mode": row.get("mode", ""),
                "failure_kind": str(extras.get("failure_kind", "")),
                "stop_reason": row.get("stop_reason", ""),
                "blocking": row.get("last_blocking_reason", ""),
                "provider": row.get("last_provider_error_kind", ""),
                "status": row.get("last_provider_http_status", ""),
            }
        )
    lines = ["## Stop Reasons", ""]
    if not detail_rows:
        return lines + ["No failed rows.", ""]
    return lines + table_rows(
        detail_rows,
        ["scenario", "mode", "failure_kind", "stop_reason", "blocking", "provider", "status"],
    )


def blocking_summary(rows: list[dict[str, str]]) -> list[str]:
    required = [row for row in rows if row.get("success") != "diagnostic_skipped"]
    if required and all(row.get("success") != "true" for row in required):
        return ["## Blocking", "", "blocking: all required runs failed", ""]
    return []


def table_rows(rows: list[dict[str, str]], headers: list[str]) -> list[str]:
    lines = ["| " + " | ".join(headers) + " |", "|" + "|".join("---" for _ in headers) + "|"]
    for row in rows:
        lines.append("| " + " | ".join(str(row.get(header, "")) for header in headers) + " |")
    lines.append("")
    return lines


def parse_extras(row: dict[str, str]) -> dict[str, object]:
    raw = row.get("extras_json", "")
    if not raw:
        return {}
    try:
        parsed = json.loads(raw)
    except json.JSONDecodeError:
        return {}
    return parsed if isinstance(parsed, dict) else {}


def compare_summaries(baseline: Path, experiment: Path) -> str:
    base = read_summary(baseline)
    exp = read_summary(experiment)
    lines = ["# Eval Compare", "", "| metric | baseline | experiment | delta |", "|---|---:|---:|---:|"]
    metrics = [
        ("success_rate", success_rate(base), success_rate(exp)),
        ("acceptance_success_rate", acceptance_rate(base), acceptance_rate(exp)),
        ("capability_acceptance_success_rate", capability_acceptance_rate(base), capability_acceptance_rate(exp)),
        ("acceptance_false_positive_count", false_positive_count(base), false_positive_count(exp)),
        (
            "p50_wall_sec",
            percentile([to_float(r.get("wall_clock_sec")) for r in base], 50),
            percentile([to_float(r.get("wall_clock_sec")) for r in exp], 50),
        ),
        (
            "p90_provider_wait_sec",
            percentile([to_float(r.get("provider_wait_sec")) for r in base], 90),
            percentile([to_float(r.get("provider_wait_sec")) for r in exp], 90),
        ),
        (
            "plan_output_adherence_score_avg",
            mean([to_float(r.get("plan_output_adherence_score")) for r in base]),
            mean([to_float(r.get("plan_output_adherence_score")) for r in exp]),
        ),
        (
            "plan_capability_contract_score_avg",
            mean([to_float(r.get("plan_capability_contract_score")) for r in base]),
            mean([to_float(r.get("plan_capability_contract_score")) for r in exp]),
        ),
        (
            "prompt_plan_capability_coverage_score_avg",
            mean([to_float(r.get("prompt_plan_capability_coverage_score")) for r in base]),
            mean([to_float(r.get("prompt_plan_capability_coverage_score")) for r in exp]),
        ),
        (
            "plan_verify_coverage_score_avg",
            mean([to_float(r.get("plan_verify_coverage_score")) for r in base]),
            mean([to_float(r.get("plan_verify_coverage_score")) for r in exp]),
        ),
        (
            "plan_verify_declared_coverage_score_avg",
            mean([to_float(r.get("plan_verify_declared_coverage_score")) for r in base]),
            mean([to_float(r.get("plan_verify_declared_coverage_score")) for r in exp]),
        ),
        (
            "executed_verify_coverage_score_avg",
            mean([to_float(r.get("executed_verify_coverage_score")) for r in base]),
            mean([to_float(r.get("executed_verify_coverage_score")) for r in exp]),
        ),
        (
            "acceptance_confidence_score_avg",
            mean([to_float(r.get("acceptance_confidence_score")) for r in base]),
            mean([to_float(r.get("acceptance_confidence_score")) for r in exp]),
        ),
        ("valid_plan_generated_rate", valid_plan_rate(base), valid_plan_rate(exp)),
        (
            "p50_exec_sec",
            percentile([to_float(r.get("exec_elapsed_sec")) for r in base], 50),
            percentile([to_float(r.get("exec_elapsed_sec")) for r in exp], 50),
        ),
        (
            "plan_quality_score_avg",
            mean([to_float(r.get("plan_quality_score")) for r in base]),
            mean([to_float(r.get("plan_quality_score")) for r in exp]),
        ),
        (
            "execution_shape_readiness_score_avg",
            mean([to_float(r.get("execution_shape_readiness_score")) for r in base]),
            mean([to_float(r.get("execution_shape_readiness_score")) for r in exp]),
        ),
        (
            "plan_run_predictive_score_avg",
            mean([to_float(r.get("plan_run_predictive_score")) for r in base]),
            mean([to_float(r.get("plan_run_predictive_score")) for r in exp]),
        ),
        (
            "plan_run_readiness_score_avg",
            mean([to_float(r.get("plan_run_readiness_score")) for r in base]),
            mean([to_float(r.get("plan_run_readiness_score")) for r in exp]),
        ),
        (
            "verify_policy_readiness_score_avg",
            mean([to_float(r.get("verify_policy_readiness_score")) for r in base]),
            mean([to_float(r.get("verify_policy_readiness_score")) for r in exp]),
        ),
        (
            "contract_handoff_score_avg",
            mean([to_float(r.get("contract_handoff_score")) for r in base]),
            mean([to_float(r.get("contract_handoff_score")) for r in exp]),
        ),
        (
            "declared_contract_completeness_score_avg",
            mean([to_float(r.get("declared_contract_completeness_score")) for r in base]),
            mean([to_float(r.get("declared_contract_completeness_score")) for r in exp]),
        ),
        (
            "runner_handoff_integrity_score_avg",
            mean([to_float(r.get("runner_handoff_integrity_score")) for r in base]),
            mean([to_float(r.get("runner_handoff_integrity_score")) for r in exp]),
        ),
        (
            "postcheck_contract_alignment_score_avg",
            mean([to_float(r.get("postcheck_contract_alignment_score")) for r in base]),
            mean([to_float(r.get("postcheck_contract_alignment_score")) for r in exp]),
        ),
        (
            "dependency_ordering_score_avg",
            mean([to_float(r.get("dependency_ordering_score")) for r in base]),
            mean([to_float(r.get("dependency_ordering_score")) for r in exp]),
        ),
        (
            "finalization_readiness_score_avg",
            mean([to_float(r.get("finalization_readiness_score")) for r in base]),
            mean([to_float(r.get("finalization_readiness_score")) for r in exp]),
        ),
        (
            "ultra_phase_readiness_min_score_avg",
            mean([to_float(r.get("ultra_phase_readiness_min_score")) for r in base]),
            mean([to_float(r.get("ultra_phase_readiness_min_score")) for r in exp]),
        ),
        (
            "plan_run_runtime_health_score_avg",
            mean([to_float(r.get("plan_run_runtime_health_score")) for r in base]),
            mean([to_float(r.get("plan_run_runtime_health_score")) for r in exp]),
        ),
        (
            "ultra_runtime_health_score_avg",
            mean([to_float(r.get("ultra_runtime_health_score")) for r in base]),
            mean([to_float(r.get("ultra_runtime_health_score")) for r in exp]),
        ),
        (
            "phase_completion_score_avg",
            mean([to_float(r.get("phase_completion_score")) for r in base]),
            mean([to_float(r.get("phase_completion_score")) for r in exp]),
        ),
        (
            "build_verify_pass_score_avg",
            mean([to_float(r.get("build_verify_pass_score")) for r in base]),
            mean([to_float(r.get("build_verify_pass_score")) for r in exp]),
        ),
        (
            "build_verifier_completion_score_avg",
            mean([to_float(r.get("build_verifier_completion_score")) for r in base]),
            mean([to_float(r.get("build_verifier_completion_score")) for r in exp]),
        ),
        (
            "dependency_setup_boundary_score_avg",
            mean([to_float(r.get("dependency_setup_boundary_score")) for r in base]),
            mean([to_float(r.get("dependency_setup_boundary_score")) for r in exp]),
        ),
        (
            "dependency_setup_bridge_score_avg",
            mean([to_float(r.get("dependency_setup_bridge_score")) for r in base]),
            mean([to_float(r.get("dependency_setup_bridge_score")) for r in exp]),
        ),
        (
            "build_verifier_lifecycle_score_avg",
            mean([to_float(r.get("build_verifier_lifecycle_score")) for r in base]),
            mean([to_float(r.get("build_verifier_lifecycle_score")) for r in exp]),
        ),
        (
            "profile_repair_symmetry_score_avg",
            mean([to_float(r.get("profile_repair_symmetry_score")) for r in base]),
            mean([to_float(r.get("profile_repair_symmetry_score")) for r in exp]),
        ),
        (
            "step_runtime_bridge_score_avg",
            mean([to_float(r.get("step_runtime_bridge_score")) for r in base]),
            mean([to_float(r.get("step_runtime_bridge_score")) for r in exp]),
        ),
        (
            "repair_target_followthrough_score_avg",
            mean([to_float(r.get("repair_target_followthrough_score")) for r in base]),
            mean([to_float(r.get("repair_target_followthrough_score")) for r in exp]),
        ),
        (
            "plan_run_success_predictor_avg",
            mean([to_float(r.get("plan_run_success_predictor")) for r in base]),
            mean([to_float(r.get("plan_run_success_predictor")) for r in exp]),
        ),
        (
            "repair_target_resolution_score_avg",
            mean([to_float(r.get("repair_target_resolution_score")) for r in base]),
            mean([to_float(r.get("repair_target_resolution_score")) for r in exp]),
        ),
        (
            "build_repair_effectiveness_score_avg",
            mean([to_float(r.get("build_repair_effectiveness_score")) for r in base]),
            mean([to_float(r.get("build_repair_effectiveness_score")) for r in exp]),
        ),
        (
            "compile_diagnostic_progress_score_avg",
            mean([to_float(r.get("compile_diagnostic_progress_score")) for r in base]),
            mean([to_float(r.get("compile_diagnostic_progress_score")) for r in exp]),
        ),
        (
            "verify_repair_edit_score_avg",
            mean([to_float(r.get("verify_repair_edit_score")) for r in base]),
            mean([to_float(r.get("verify_repair_edit_score")) for r in exp]),
        ),
        (
            "prompt_contract_score_avg",
            mean([to_float(r.get("prompt_contract_score")) for r in base]),
            mean([to_float(r.get("prompt_contract_score")) for r in exp]),
        ),
        (
            "step_obligation_scope_score_avg",
            mean([to_float(r.get("step_obligation_scope_score")) for r in base]),
            mean([to_float(r.get("step_obligation_scope_score")) for r in exp]),
        ),
        (
            "dependency_contract_score_avg",
            mean([to_float(r.get("dependency_contract_score")) for r in base]),
            mean([to_float(r.get("dependency_contract_score")) for r in exp]),
        ),
        (
            "config_contract_score_avg",
            mean([to_float(r.get("config_contract_score")) for r in base]),
            mean([to_float(r.get("config_contract_score")) for r in exp]),
        ),
        (
            "verify_contract_score_avg",
            mean([to_float(r.get("verify_contract_score")) for r in base]),
            mean([to_float(r.get("verify_contract_score")) for r in exp]),
        ),
        (
            "postcheck_stability_score_avg",
            mean([to_float(r.get("postcheck_stability_score")) for r in base]),
            mean([to_float(r.get("postcheck_stability_score")) for r in exp]),
        ),
        (
            "execution_contract_adherence_score_avg",
            mean([to_float(r.get("execution_contract_adherence_score")) for r in base]),
            mean([to_float(r.get("execution_contract_adherence_score")) for r in exp]),
        ),
        (
            "executable_plan_score_avg",
            mean([to_float(r.get("executable_plan_score")) for r in base]),
            mean([to_float(r.get("executable_plan_score")) for r in exp]),
        ),
        (
            "constraint_coverage_score_avg",
            mean([to_float(r.get("constraint_coverage_score")) for r in base]),
            mean([to_float(r.get("constraint_coverage_score")) for r in exp]),
        ),
        (
            "verify_strength_score_avg",
            mean([to_float(r.get("verify_strength_score")) for r in base]),
            mean([to_float(r.get("verify_strength_score")) for r in exp]),
        ),
        (
            "verify_adequacy_score_avg",
            mean([to_float(r.get("verify_adequacy_score")) for r in base]),
            mean([to_float(r.get("verify_adequacy_score")) for r in exp]),
        ),
        (
            "semantic_verify_coverage_score_avg",
            mean([to_float(r.get("semantic_verify_coverage_score")) for r in base]),
            mean([to_float(r.get("semantic_verify_coverage_score")) for r in exp]),
        ),
        (
            "behavior_oracle_declared_score_avg",
            mean([to_float(r.get("behavior_oracle_declared_score")) for r in base]),
            mean([to_float(r.get("behavior_oracle_declared_score")) for r in exp]),
        ),
        (
            "contentless_verify_penalty_avg",
            mean([to_float(r.get("contentless_verify_penalty")) for r in base]),
            mean([to_float(r.get("contentless_verify_penalty")) for r in exp]),
        ),
        (
            "artifact_ownership_score_avg",
            mean([to_float(r.get("artifact_ownership_score")) for r in base]),
            mean([to_float(r.get("artifact_ownership_score")) for r in exp]),
        ),
        (
            "lint_repair_score_avg",
            mean([to_float(r.get("lint_repair_score")) for r in base]),
            mean([to_float(r.get("lint_repair_score")) for r in exp]),
        ),
        (
            "stability_score_avg",
            mean([to_float(r.get("stability_score")) for r in base]),
            mean([to_float(r.get("stability_score")) for r in exp]),
        ),
        (
            "runtime_friction_score_avg",
            mean([to_float(r.get("runtime_friction_score")) for r in base]),
            mean([to_float(r.get("runtime_friction_score")) for r in exp]),
        ),
        (
            "runtime_friction_raw_score_avg",
            mean([to_float(r.get("runtime_friction_raw_score")) for r in base]),
            mean([to_float(r.get("runtime_friction_raw_score")) for r in exp]),
        ),
        (
            "artifact_progress_score_avg",
            mean([to_float(r.get("artifact_progress_score")) for r in base]),
            mean([to_float(r.get("artifact_progress_score")) for r in exp]),
        ),
        (
            "finalization_score_avg",
            mean([to_float(r.get("finalization_score")) for r in base]),
            mean([to_float(r.get("finalization_score")) for r in exp]),
        ),
        (
            "step_finalization_score_avg",
            mean([to_float(r.get("step_finalization_score")) for r in base]),
            mean([to_float(r.get("step_finalization_score")) for r in exp]),
        ),
        (
            "plan_finalization_score_avg",
            mean([to_float(r.get("plan_finalization_score")) for r in base]),
            mean([to_float(r.get("plan_finalization_score")) for r in exp]),
        ),
        (
            "deferred_verify_finalization_score_avg",
            mean([to_float(r.get("deferred_verify_finalization_score")) for r in base]),
            mean([to_float(r.get("deferred_verify_finalization_score")) for r in exp]),
        ),
        (
            "postcheck_finalization_score_avg",
            mean([to_float(r.get("postcheck_finalization_score")) for r in base]),
            mean([to_float(r.get("postcheck_finalization_score")) for r in exp]),
        ),
        (
            "tool_policy_compatibility_score_avg",
            mean([to_float(r.get("tool_policy_compatibility_score")) for r in base]),
            mean([to_float(r.get("tool_policy_compatibility_score")) for r in exp]),
        ),
        (
            "execution_contract_adherence_raw_score_avg",
            mean([to_float(r.get("execution_contract_adherence_raw_score")) for r in base]),
            mean([to_float(r.get("execution_contract_adherence_raw_score")) for r in exp]),
        ),
        (
            "execution_contract_min_subscore_avg",
            mean([to_float(r.get("execution_contract_min_subscore")) for r in base]),
            mean([to_float(r.get("execution_contract_min_subscore")) for r in exp]),
        ),
        (
            "overall_score_avg",
            mean([to_float(r.get("overall_score")) for r in base]),
            mean([to_float(r.get("overall_score")) for r in exp]),
        ),
    ]
    for name, b, e in metrics:
        delta = None if b is None or e is None else e - b
        lines.append(f"| {name} | {fmt(b)} | {fmt(e)} | {fmt(delta)} |")
    lines.append("")
    return "\n".join(lines)


def success_rate(rows: list[dict[str, str]]) -> float:
    if not rows:
        return 0.0
    return 100.0 * sum(1 for row in rows if row.get("success") == "true") / len(rows)


def acceptance_rate(rows: list[dict[str, str]]) -> float | None:
    scoped = [row for row in rows if row.get("acceptance_success") not in {"", None}]
    if not scoped:
        return None
    return 100.0 * sum(1 for row in scoped if row.get("acceptance_success") == "true") / len(scoped)


def capability_acceptance_rate(rows: list[dict[str, str]]) -> float | None:
    scoped = [row for row in rows if row.get("capability_acceptance_success") not in {"", None}]
    if not scoped:
        return None
    return 100.0 * sum(1 for row in scoped if row.get("capability_acceptance_success") == "true") / len(scoped)


def false_positive_count(rows: list[dict[str, str]]) -> float:
    return float(sum(1 for row in rows if row.get("acceptance_false_positive") == "true"))


def valid_plan_rate(rows: list[dict[str, str]]) -> float | None:
    scoped = [row for row in rows if row.get("valid_plan_generated") not in {"", None}]
    if not scoped:
        return None
    return 100.0 * sum(1 for row in scoped if row.get("valid_plan_generated") == "true") / len(scoped)


def success_count_cell(rows: list[dict[str, str]]) -> str:
    return f"{sum(1 for row in rows if row.get('success') == 'true')}/{len(rows)}"


def acceptance_count_cell(rows: list[dict[str, str]]) -> str:
    scoped = [row for row in rows if row.get("acceptance_success") not in {"", None}]
    if not scoped:
        return "n/a"
    return f"{sum(1 for row in scoped if row.get('acceptance_success') == 'true')}/{len(scoped)}"


def capability_acceptance_count_cell(rows: list[dict[str, str]]) -> str:
    scoped = [row for row in rows if row.get("capability_acceptance_success") not in {"", None}]
    if not scoped:
        return "n/a"
    return f"{sum(1 for row in scoped if row.get('capability_acceptance_success') == 'true')}/{len(scoped)}"


def valid_plan_count_cell(rows: list[dict[str, str]]) -> str:
    scoped = [row for row in rows if row.get("valid_plan_generated") not in {"", None}]
    if not scoped:
        return ""
    return f"{sum(1 for row in scoped if row.get('valid_plan_generated') == 'true')}/{len(scoped)}"


def prediction_key(row: dict[str, str]) -> tuple[str, str, str, str, str, str]:
    return (
        row.get("scenario", ""),
        row.get("main_provider", ""),
        row.get("main_model", ""),
        row.get("planner_provider", ""),
        row.get("planner_model", ""),
        row.get("local_llm_used", ""),
    )


def group_by_prediction_key(rows: list[dict[str, str]]) -> dict[tuple[str, str, str, str, str, str], list[dict[str, str]]]:
    groups: dict[tuple[str, str, str, str, str, str], list[dict[str, str]]] = defaultdict(list)
    for row in rows:
        groups[prediction_key(row)].append(row)
    return groups


def group_rows(rows: list[dict[str, str]], key: str) -> dict[str, list[dict[str, str]]]:
    groups: dict[str, list[dict[str, str]]] = defaultdict(list)
    for row in rows:
        groups[row.get(key, "")].append(row)
    return groups


def most_common(rows: list[dict[str, str]], key: str) -> str:
    counter = Counter(str(row.get(key, "")) for row in rows if row.get(key) not in {"", None})
    if not counter:
        return ""
    return counter.most_common(1)[0][0]


def capability_cell(row: dict[str, str]) -> str:
    extras = parse_extras(row)
    return str(capability_failure_included(str(extras.get("failure_kind", "")))).lower()


def split_reasons(raw: str | None) -> list[str]:
    if not raw:
        return []
    text = str(raw)
    if text.startswith("["):
        try:
            parsed = json.loads(text)
        except json.JSONDecodeError:
            parsed = None
        if isinstance(parsed, list):
            return [str(part) for part in parsed if str(part)]
    return [part for part in str(raw).split(";") if part]


def to_float(value: str | None) -> float | None:
    if value is None or value in {"", "not_available"}:
        return None
    try:
        return float(value)
    except ValueError:
        return None


def safe_int(value: object) -> int:
    if value is None or value == "":
        return 0
    try:
        return int(str(value))
    except ValueError:
        return 0


def mean(values: Iterable[float | None]) -> float | None:
    clean = [value for value in values if value is not None]
    return statistics.fmean(clean) if clean else None


def percentile(values: Iterable[float | None], pct: int) -> float | None:
    clean = sorted(value for value in values if value is not None)
    if not clean:
        return None
    if len(clean) == 1:
        return clean[0]
    index = round((pct / 100) * (len(clean) - 1))
    return clean[index]


def pearson(xs: list[float], ys: list[float]) -> float | None:
    if len(xs) < 2 or len(xs) != len(ys):
        return None
    mean_x = statistics.fmean(xs)
    mean_y = statistics.fmean(ys)
    numerator = sum((x - mean_x) * (y - mean_y) for x, y in zip(xs, ys))
    denom_x = sum((x - mean_x) ** 2 for x in xs)
    denom_y = sum((y - mean_y) ** 2 for y in ys)
    if denom_x == 0 or denom_y == 0:
        return None
    return numerator / (denom_x * denom_y) ** 0.5


def fmt(value: float | None) -> str:
    if value is None:
        return ""
    return f"{value:.1f}"
