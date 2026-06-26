import tempfile
import unittest
from pathlib import Path
import sys

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "scripts"))

from eval_lib.failure_classification import classify_events
from eval_lib.report import generate_report
from eval_lib.run_summary import SUMMARY_HEADER, write_summary


class PlanQualityReportTest(unittest.TestCase):
    def test_quality_warning_does_not_become_failure_classification(self):
        classified = classify_events(
            [
                {
                    "event": "planner_quality_warning",
                    "planner_stage": "quality",
                    "planner_error_kind": "planner_quality_warning",
                    "planner_error_message": "large task has one step",
                }
            ]
        )
        self.assertEqual(classified, {})

    def test_report_summarizes_quality_warnings(self):
        with tempfile.TemporaryDirectory() as td:
            run_root = Path(td)
            row = {key: "" for key in SUMMARY_HEADER}
            row.update(
                {
                    "run_id": "quality",
                    "suite": "s",
                    "scenario": "scenario",
                    "size": "large",
                    "category": "planner",
                    "mode": "step-plan",
                    "planner_provider": "openai",
                    "planner_model": "gpt-5.4-mini",
                    "success": "true",
                    "rc": "0",
                    "plan_quality_score": "82",
                    "executable_plan_score": "41",
                    "constraint_coverage_score": "73",
                    "verify_strength_score": "25",
                    "artifact_ownership_score": "88",
                    "lint_repair_score": "90",
                    "extras_json": {
                        "planner_quality_warnings": [
                            {
                                "planner_provider": "openai",
                                "planner_model": "gpt-5.4-mini",
                                "message": "large task is represented as a single step",
                            }
                        ]
                    },
                }
            )
            write_summary(run_root / "summary.eval.tsv", [row])
            report = generate_report(run_root)
        self.assertIn("## Planner Quality Warnings", report)
        self.assertIn("## Executable Plan Quality", report)
        self.assertIn("## Additional Plan Metrics", report)
        self.assertIn("## Stability", report)
        self.assertIn("## Plan Run Predictiveness", report)
        self.assertIn("| bottom | scenario | step-plan | 41.0 | 82 | true |", report)
        self.assertIn("large task is represented as a single step", report)
        self.assertIn("| openai | gpt-5.4-mini |", report)

    def test_report_summarizes_predictiveness_pair(self):
        with tempfile.TemporaryDirectory() as td:
            run_root = Path(td)
            rows = []
            for mode, success, overall in [("step-plan", "true", "88"), ("plan-run", "false", "52")]:
                row = {key: "" for key in SUMMARY_HEADER}
                row.update(
                    {
                        "run_id": mode,
                        "suite": "s",
                        "scenario": "scenario",
                        "size": "small",
                        "category": "planner",
                        "mode": mode,
                        "main_provider": "openai",
                        "main_model": "gpt-5.4-mini",
                        "planner_provider": "gemini",
                        "planner_model": "gemini-3.5-flash",
                        "local_llm_used": "false",
                        "success": success,
                        "overall_score": overall,
                        "stability_score": "91",
                    }
                )
                rows.append(row)
            write_summary(run_root / "summary.eval.tsv", rows)
            report = generate_report(run_root)
        self.assertIn("## Plan Run Predictiveness", report)
        self.assertIn("false_positive: 1", report)
        self.assertIn("## Stability", report)
        self.assertIn("91.0", report)


if __name__ == "__main__":
    unittest.main()
