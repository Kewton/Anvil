#!/usr/bin/env python3
from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path

from eval_lib.failure_classification import (
    capability_failure_included,
    failure_layer_for_kind,
    read_jsonl,
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
        rescored.append(updated)

    out_summary = Path(args.out_summary)
    write_summary(out_summary, rescored)
    print(out_summary)
    return 0


def failure_kind_from_row(row: dict[str, str]) -> str:
    try:
        extras = json.loads(row.get("extras_json", "") or "{}")
    except json.JSONDecodeError:
        return ""
    if isinstance(extras, dict):
        return str(extras.get("failure_kind", ""))
    return ""


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
