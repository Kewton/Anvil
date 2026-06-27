#!/usr/bin/env python3
from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path

from eval_lib.acceptance_outcome import evaluate_acceptance_outcome
from eval_lib.failure_classification import (
    capability_failure_included,
    failure_layer_for_kind,
    read_jsonl,
)
from eval_lib.plan_readiness import (
    READINESS_FIELDS,
    aggregate_ultra_phase_readiness,
    classify_readiness_outcome,
    empty_plan_readiness_scores,
    score_plan_readiness_file,
)
from eval_lib.run_summary import read_summary, write_summary
from eval_lib.runtime_scoring import score_runtime_health
from eval_lib.suites import load_suite


def main() -> int:
    parser = argparse.ArgumentParser(description="Post-hoc rescore eval runtime metrics.")
    parser.add_argument("--run-root", required=True)
    parser.add_argument("--suite", required=True)
    parser.add_argument("--out-summary", required=True)
    args = parser.parse_args()

    run_root = Path(args.run_root)
    scenarios = {
        scenario["id"]: scenario
        for scenario in load_suite(args.suite)["scenarios"]
    }
    rows = read_summary(run_root / "summary.eval.tsv")
    rescored = []
    for row in rows:
        updated = dict(row)
        scenario = scenarios.get(row.get("scenario", ""), {})
        run_dir = run_root / "runs" / str(row.get("run_id", ""))
        events = read_jsonl(run_dir / "anvil-events.jsonl")
        workdir = Path(row.get("workdir", "")) if row.get("workdir") else run_dir
        plan_paths = [
            run_dir / path
            for path in str(row.get("plan_artifacts", "")).split(",")
            if path
        ]
        readiness = score_readiness_for_row(
            row,
            scenario=scenario,
            events=events,
            plan_paths=plan_paths,
        )
        updated.update(readiness)
        if events and scenario:
            runtime_scores = score_runtime_health(
                events,
                mode=row.get("mode", ""),
                success=row.get("success") == "true",
                scenario=scenario,
                workdir=workdir,
                plan_paths=plan_paths,
                run_dir=run_dir,
            )
            updated.update(runtime_scores)
            postcheck_events = run_dir / "postcheck" / "events.jsonl"
            acceptance = evaluate_acceptance_outcome(
                scenario=scenario,
                workdir=workdir,
                run_dir=run_dir,
                mode=row.get("mode", ""),
                process_success=row.get("rc") == "0",
                legacy_success=row.get("success") == "true",
                postcheck={
                    "ok": row.get("success") == "true",
                    "events_path": str(postcheck_events) if postcheck_events.exists() else "",
                },
                plan_paths=plan_paths,
            )
            updated.update(
                {
                    key: value
                    for key, value in acceptance.items()
                    if key != "acceptance_details"
                }
            )
            if acceptance.get("acceptance_details"):
                extras = parse_extras_dict(updated.get("extras_json", ""))
                extras["acceptance_details"] = acceptance["acceptance_details"]
                updated["extras_json"] = extras
            if row.get("mode", "") in {"minimal-loop", "plan-run", "ultra-plan-run", "ultra-step-run"} and not any(
                runtime_scores.get(key) not in {"", None}
                for key in [
                    "runtime_friction_score",
                    "finalization_score",
                    "phase_completion_score",
                    "execution_contract_adherence_score",
                ]
            ):
                mark_unavailable(updated)
        else:
            mark_unavailable(updated)

        failure_kind = failure_kind_from_row(updated)
        if failure_kind:
            updated["failure_layer"] = failure_layer_for_kind(failure_kind)
            included = capability_failure_included(failure_kind)
            updated["capability_failure_included"] = str(included).lower() if included != "" else ""
        readiness_outcome = classify_readiness_outcome(
            updated.get("plan_run_readiness_score", ""),
            success=updated.get("success") == "true",
            failure_kind=failure_kind,
        )
        updated.update(readiness_outcome)
        rescored.append(updated)

    out_summary = Path(args.out_summary)
    write_summary(out_summary, rescored)
    print(out_summary)
    return 0


def failure_kind_from_row(row: dict[str, str]) -> str:
    extras = parse_extras_dict(row.get("extras_json", ""))
    if isinstance(extras, dict):
        return str(extras.get("failure_kind", ""))
    return ""


def parse_extras_dict(raw: object) -> dict[str, object]:
    if isinstance(raw, dict):
        return dict(raw)
    try:
        parsed = json.loads(str(raw or "{}"))
    except json.JSONDecodeError:
        return {}
    return parsed if isinstance(parsed, dict) else {}


def score_readiness_for_row(
    row: dict[str, str],
    *,
    scenario: dict,
    events: list[dict],
    plan_paths: list[Path],
) -> dict[str, object]:
    out = empty_plan_readiness_scores()
    phase_scores = []
    for path in plan_paths:
        if not path.exists():
            continue
        readiness = score_plan_readiness_file(
            path,
            profile=str(scenario.get("profile", "")),
            prompt=str(scenario.get("prompt", "")),
            handoff_events=events,
        )
        if readiness.get("plan_run_readiness_score") in {"", None}:
            continue
        phase_scores.append(readiness)
        out.update({key: readiness.get(key, "") for key in READINESS_FIELDS if key in readiness})
    ultra_phase = aggregate_ultra_phase_readiness(phase_scores)
    if row.get("mode") == "ultra-plan-run" and ultra_phase.get("ultra_phase_readiness_min_score") not in {"", None}:
        out.update(ultra_phase)
        out["plan_run_readiness_score"] = ultra_phase["ultra_phase_readiness_min_score"]
        out["readiness_cap_reason"] = ultra_phase["ultra_phase_readiness_cap_reason"]
    return out


def mark_unavailable(row: dict[str, str]) -> None:
    for key in [
        "runtime_friction_raw_score",
        "runtime_friction_reason",
        "finalization_reason",
        "postcheck_stability_reason",
        "execution_contract_adherence_raw_score",
        "execution_contract_min_subscore",
        "execution_contract_cap_reason",
        "phase_plan_validity_score",
        "phase_scaffold_success_score",
        "phase_step_execution_score",
        "phase_verify_success_score",
        "phase_postcheck_success_score",
        "phase_finalization_score",
        "phase_failure_stage",
    ]:
        if not row.get(key):
            row[key] = "not_available"


if __name__ == "__main__":
    sys.exit(main())
