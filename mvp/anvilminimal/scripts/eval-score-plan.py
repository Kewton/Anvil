#!/usr/bin/env python3
from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path

from eval_lib.artifacts import append_jsonl
from eval_lib.plan_scoring import score_plan_file
from eval_lib.suites import load_suite


def main() -> int:
    parser = argparse.ArgumentParser(description="Score step plan or ultra plan artifacts.")
    parser.add_argument("--plan")
    parser.add_argument("--run-root")
    parser.add_argument("--suite", default="eval/suites/mvp-smoke.yaml")
    parser.add_argument("--scenario-id")
    parser.add_argument("--rules", default="eval/scoring_rules.yaml")
    args = parser.parse_args()
    scenario = find_scenario(args.suite, args.scenario_id) if args.scenario_id else None
    if args.plan:
        score = score_plan_file(args.plan, scenario)
        print(json.dumps(score, ensure_ascii=False, indent=2, sort_keys=True))
        return 0 if score["score"] > 0 else 1
    if args.run_root:
        root = Path(args.run_root)
        count = 0
        for plan in sorted((root / "runs").glob("*/plans/*.yaml")):
            score = score_plan_file(plan, scenario)
            append_jsonl(root / "events.jsonl", {"event": "plan_score", "plan": str(plan), **score})
            count += 1
        print(json.dumps({"run_root": str(root), "plans_scored": count}, indent=2))
        return 0
    parser.error("one of --plan or --run-root is required")
    return 2


def find_scenario(suite_path: str, scenario_id: str | None) -> dict | None:
    suite = load_suite(suite_path)
    for scenario in suite["scenarios"]:
        if scenario["id"] == scenario_id:
            return scenario
    return None


if __name__ == "__main__":
    sys.exit(main())

