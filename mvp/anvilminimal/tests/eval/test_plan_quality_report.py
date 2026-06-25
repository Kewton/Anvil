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
        self.assertIn("large task is represented as a single step", report)
        self.assertIn("| openai | gpt-5.4-mini |", report)


if __name__ == "__main__":
    unittest.main()
